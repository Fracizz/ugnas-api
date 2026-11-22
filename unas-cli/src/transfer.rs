use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use bytes::Bytes;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::blocking::{Response, multipart};
use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, CONTENT_DISPOSITION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_TYPE, ETAG, HeaderMap, HeaderValue, IF_RANGE, LAST_MODIFIED, LOCATION, RANGE,
};
use reqwest::{Method, StatusCode, Url};
use same_file::Handle;
use serde_json::{Value, json};
use tempfile::{NamedTempFile, TempPath};
use uuid::Uuid;

use crate::client::{Error, NasClient, Result, data};
use crate::ops;

const CHUNK_SIZE: usize = 8 * 1024 * 1024;
const DOWNLOAD_BUFFER: usize = 256 * 1024;
const API_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_UPLOAD_RETRIES: u8 = 3;
const URL_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

struct Progress {
    enabled: bool,
    last_update: Option<Instant>,
}

impl Progress {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            last_update: None,
        }
    }

    fn report(&mut self, done: u64, total: Option<u64>, final_update: bool) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        if !final_update
            && self
                .last_update
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(100))
        {
            return;
        }
        self.last_update = Some(now);
        let mut stderr = io::stderr().lock();
        if let Some(total) = total {
            let _ = write!(stderr, "\rtransferred {done}/{total} bytes");
        } else {
            let _ = write!(stderr, "\rtransferred {done} bytes");
        }
        let _ = stderr.flush();
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        if self.last_update.is_some() {
            let _ = writeln!(io::stderr());
        }
    }
}

struct Upload {
    id: String,
    dir: String,
    name: String,
    total: u64,
    modified_nanos: i128,
    action_type: u8,
}

impl Upload {
    fn prepare(&self, client: &mut NasClient) -> Result<Value> {
        let metadata = [
            ("uuid", self.id.clone()),
            ("dir", self.dir.clone()),
            ("action_type", self.action_type.to_string()),
            ("size", self.total.to_string()),
            ("begin_size", "0".into()),
            ("current_size", "0".into()),
            (
                "change_time",
                (self.modified_nanos / 1_000_000_000).to_string(),
            ),
            ("filename", self.name.clone()),
            ("resume", "true".into()),
            ("first_request", "true".into()),
        ];
        let filename = utf8_percent_encode(&self.name, URL_COMPONENT).to_string();
        client.request(Method::POST, "/ugreen/v1/filemgr/fileUpload", |request| {
            let file = multipart::Part::bytes(Vec::new())
                .file_name(filename.clone())
                .mime_str("application/octet-stream")
                .expect("static MIME type is valid");
            let form = metadata
                .iter()
                .fold(multipart::Form::new(), |form, (key, value)| {
                    form.text(*key, value.clone())
                })
                .part("file", file);
            request.timeout(TRANSFER_TIMEOUT).multipart(form)
        })
    }

    fn offset(&self, client: &mut NasClient) -> Result<Option<u64>> {
        let size = self.total.to_string();
        let payload = match client.request(
            Method::GET,
            "/ugreen/v1/filemgr/getUpdateTmpInfo",
            |request| {
                request.timeout(API_TIMEOUT).query(&[
                    ("dir", self.dir.as_str()),
                    ("uuid", self.id.as_str()),
                    ("size", size.as_str()),
                ])
            },
        ) {
            Ok(payload) => payload,
            Err(error) if matches!(error.status(), Some(404 | 405)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let data = data(&payload);
        let offset = data.get("result").unwrap_or(data);
        if offset.is_null() {
            return Ok(None);
        }
        let parsed = offset
            .as_u64()
            .or_else(|| offset.as_str()?.parse::<u64>().ok());
        match parsed {
            Some(offset) if offset <= self.total => Ok(Some(offset)),
            Some(_) => Err(Error::Other(
                "NAS upload offset exceeds the source file size".into(),
            )),
            None if offset.is_object() => Ok(None),
            None => Err(Error::Other("NAS returned an invalid upload offset".into())),
        }
    }

    fn send(&self, client: &mut NasClient, offset: u64, chunk: &Bytes) -> Result<Value> {
        let parameters = json!({
            "uuid": self.id,
            "file_name": self.name,
            "action_type": self.action_type,
            "size": self.total,
            "current_size": chunk.len(),
            "resume": true,
            "dir": utf8_percent_encode(&self.dir, URL_COMPONENT).to_string(),
            "change_time": self.modified_nanos / 1_000_000_000,
            "is_live_photo": false,
            "first_request": false,
            "begin_size": offset,
        });
        let parameters = HeaderValue::from_bytes(parameters.to_string().as_bytes())
            .map_err(|_| Error::Usage("invalid upload metadata".into()))?;
        let disposition = format!(
            "attachment; filename=\"{}\"",
            utf8_percent_encode(&self.name, URL_COMPONENT)
        );
        client.request(Method::POST, "/ugreen/v1/filemgr/fileUploadV2", |request| {
            request
                .timeout(TRANSFER_TIMEOUT)
                .header(CONTENT_TYPE, "application/octet-stream")
                .header(CONTENT_DISPOSITION, &disposition)
                .header("ug-param", parameters.clone())
                .body(chunk.clone())
        })
    }

    fn result(&self, response: &Value) -> Value {
        let result = data(response);
        let remote_path = result
            .get("result")
            .and_then(Value::as_str)
            .filter(|path| path.starts_with('/'))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{}/{}", self.dir, self.name));
        json!({ "path": remote_path, "result": result })
    }
}

fn file_modified_nanos(metadata: &fs::Metadata) -> Result<i128> {
    Ok(match metadata.modified()?.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(before_epoch) => -(before_epoch.duration().as_nanos() as i128),
    })
}

fn checked_name(name: &str) -> Result<&str> {
    if matches!(name, "" | "." | "..")
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(Error::Usage(
            "expected a filename without path separators or control characters".into(),
        ));
    }
    Ok(name)
}

pub(crate) fn upload(
    client: &mut NasClient,
    src: &Path,
    dest_dir: &str,
    name: Option<&str>,
    force: bool,
    progress: bool,
) -> Result<Value> {
    if !fs::metadata(src)?.is_file() {
        return Err(Error::Usage(format!(
            "upload source is not a file: {}",
            src.display()
        )));
    }
    let mut file = File::open(src)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(Error::Usage("upload source is not a regular file".into()));
    }
    let name = name
        .or_else(|| src.file_name().and_then(OsStr::to_str))
        .ok_or_else(|| {
            Error::Usage("pass --name for a source with a non-Unicode filename".into())
        })?;
    let name = checked_name(name)?;
    let dir = dest_dir.replace('\\', "/");
    let dir = dir.trim_end_matches('/');
    if !dir.starts_with('/') {
        return Err(Error::Usage(
            "upload destination must be an absolute NAS directory".into(),
        ));
    }
    let total = metadata.len();
    let modified_nanos = file_modified_nanos(&metadata)?;
    let canonical = src.canonicalize()?;
    let resolved = canonical.to_string_lossy();
    let fingerprint = format!(
        "{}|{resolved}|{total}|{modified_nanos}|{dir}|{name}",
        client.client_id()
    );
    let id = format!(
        "{}_1_{:x}",
        Uuid::new_v5(&Uuid::NAMESPACE_URL, fingerprint.as_bytes()),
        md5::compute(format!("{resolved}{total}")),
    );
    let upload = Upload {
        id,
        dir: dir.into(),
        name: name.into(),
        total,
        modified_nanos,
        action_type: u8::from(force),
    };
    ops::mkdir(client, dir)?;
    let mut last = upload.prepare(client)?;
    if match &data(&last)["result"] {
        Value::Bool(result) => *result,
        Value::String(result) => !result.is_empty(),
        _ => false,
    } {
        let mut indicator = Progress::new(progress);
        indicator.report(total, Some(total), true);
        return Ok(upload.result(&last));
    }

    let mut offset = upload.offset(client)?.unwrap_or(0);
    let mut retries = 0;
    let mut send_empty = total == 0;
    let mut indicator = Progress::new(progress);
    while offset < total || send_empty {
        file.seek(SeekFrom::Start(offset))?;
        let mut chunk = vec![0; (total - offset).min(CHUNK_SIZE as u64) as usize];
        file.read_exact(&mut chunk)?;
        let current = file.metadata()?;
        if current.len() != total || file_modified_nanos(&current)? != modified_nanos {
            return Err(Error::Other("upload source changed during transfer".into()));
        }
        let chunk = Bytes::from(chunk);
        match upload.send(client, offset, &chunk) {
            Ok(response) => {
                last = response;
                offset += chunk.len() as u64;
                retries = 0;
                send_empty = false;
                indicator.report(offset, Some(total), false);
            }
            Err(Error::Network(_)) if retries < MAX_UPLOAD_RETRIES => {
                retries += 1;
                thread::sleep(Duration::from_millis(250 * (1 << retries)));
                offset = upload.offset(client)?.unwrap_or(offset);
            }
            Err(error) => return Err(error),
        }
    }
    indicator.report(total, Some(total), true);
    Ok(upload.result(&last))
}

#[derive(Clone, Copy)]
pub(crate) struct DownloadOptions<'a> {
    pub(crate) dest: &'a Path,
    pub(crate) dry_run: bool,
    pub(crate) overwrite: bool,
    pub(crate) progress: bool,
}

struct DownloadTarget {
    url: Url,
    header_token: Option<String>,
    public: bool,
}

fn nas_download_target(client: &mut NasClient, nas_path: &str) -> Result<DownloadTarget> {
    let paths = utf8_percent_encode(nas_path, URL_COMPONENT);
    let endpoint = format!("/ugreen/v2/filemgr/getDownloadToken?paths={paths}");
    let token = client.request(Method::GET, &endpoint, |request| {
        request.query(&[("intranet_share_id", "0"), ("coding", "true")])
    });
    match token {
        Ok(payload) => {
            let token = data(&payload)["dl_token"]
                .as_str()
                .filter(|token| !token.is_empty())
                .ok_or_else(|| Error::Other("NAS did not return a download token".into()))?;
            let mut url = client.url("/ugreen/v2/filemgr/downloadFile")?;
            url.query_pairs_mut().append_pair("token", token);
            Ok(DownloadTarget {
                url,
                header_token: None,
                public: false,
            })
        }
        Err(error) if matches!(error.status(), Some(404 | 405)) => {
            let mut url = client.url(&format!(
                "/ugreen/v1/filemgr/downloadFile?intranet_share_id=0&paths={paths}"
            ))?;
            let mut header_token = None;
            if let Some(token) = client.token() {
                if client.token_in_url() {
                    url.query_pairs_mut().append_pair("token", token);
                } else {
                    header_token = Some(token.to_owned());
                }
            }
            Ok(DownloadTarget {
                url,
                header_token,
                public: false,
            })
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn download(
    client: &mut NasClient,
    nas_path: &str,
    options: DownloadOptions<'_>,
) -> Result<Value> {
    let logical_resource = format!(
        "nas\0{}\0{}\0{nas_path}",
        client.url("/")?,
        client.client_id()
    );
    let target = nas_download_target(client, nas_path)?;
    match receive(client, &target, &logical_resource, Some(nas_path), options) {
        Err(error) if error.status() == Some(401) => {
            let fresh = nas_download_target(client, nas_path)?;
            receive(client, &fresh, &logical_resource, Some(nas_path), options)
        }
        result => result,
    }
}

pub(crate) fn download_public(
    client: &NasClient,
    url: &str,
    identity: &str,
    source_name: Option<&str>,
    options: DownloadOptions<'_>,
) -> Result<Value> {
    let url = Url::parse(url).map_err(|_| Error::Usage("invalid share download URL".into()))?;
    validate_http_url(&url)?;
    let mut resource = url.clone();
    resource.set_query(None);
    resource.set_fragment(None);
    let logical_resource = format!("share\0{resource}\0{identity}");
    receive(
        client,
        &DownloadTarget {
            url,
            header_token: None,
            public: true,
        },
        &logical_resource,
        source_name,
        options,
    )
}

fn validate_http_url(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Error::Usage(
            "download URL must be HTTP or HTTPS without inline credentials".into(),
        ));
    }
    Ok(())
}

fn target_path(dest: &Path, source_name: Option<&str>) -> Result<PathBuf> {
    let display = dest.as_os_str().to_string_lossy();
    let directory =
        dest.is_dir() || display.ends_with('/') || (cfg!(windows) && display.ends_with('\\'));
    if !directory {
        if dest.file_name().is_none() {
            return Err(Error::Usage(
                "download destination must specify a file or directory".into(),
            ));
        }
        return Ok(dest.to_path_buf());
    }
    let name = source_name
        .and_then(|name| {
            name.trim_end_matches(['/', '\\'])
                .rsplit(['/', '\\'])
                .next()
        })
        .ok_or_else(|| {
            Error::Usage("unable to infer a filename; specify the destination file path".into())
        })?;
    let name = checked_name(name)?;
    if cfg!(windows) {
        let stem = name
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_ascii_uppercase();
        let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                stem.strip_prefix(prefix).is_some_and(|digit| {
                    matches!(
                        digit,
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
                })
            });
        if name.contains([':', '<', '>', '"', '|', '?', '*'])
            || name.ends_with(['.', ' '])
            || device
        {
            return Err(Error::Usage(
                "source filename is invalid on Windows; specify the destination file path".into(),
            ));
        }
    }
    Ok(dest.join(name))
}

fn stream(
    client: &NasClient,
    target: &DownloadTarget,
    method: Method,
    range: Option<String>,
    validator: Option<&HeaderValue>,
    timeout: Option<Duration>,
) -> Result<Response> {
    let mut url = target.url.clone();
    for _ in 0..=5 {
        let mut request = client
            .http()
            .request(method.clone(), url.clone())
            .header(ACCEPT, "*/*")
            .header(ACCEPT_ENCODING, "identity");
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        if !target.public {
            request = request
                .header("UG-Agent", "PC/WEB")
                .header("Client-Id", client.client_id())
                .header("Client-Version", "1.18.0");
        }
        if let Some(range) = &range {
            request = request.header(RANGE, range);
            if let Some(validator) = validator {
                request = request.header(IF_RANGE, validator.clone());
            }
        }
        if let Some(token) = &target.header_token {
            let mut token = HeaderValue::from_str(token)
                .map_err(|_| Error::Auth("invalid NAS session token".into()))?;
            token.set_sensitive(true);
            request = request.header("X-Ugreen-Token", token);
        }
        let response = request.send()?;
        if !matches!(
            response.status(),
            StatusCode::MOVED_PERMANENTLY
                | StatusCode::FOUND
                | StatusCode::SEE_OTHER
                | StatusCode::TEMPORARY_REDIRECT
                | StatusCode::PERMANENT_REDIRECT
        ) {
            return Ok(response);
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                Error::Other("download redirect did not provide a valid destination".into())
            })?;
        let next = url
            .join(location)
            .map_err(|_| Error::Other("invalid download redirect".into()))?;
        validate_http_url(&next)?;
        if (url.scheme() == "https" && next.scheme() != "https")
            || (!target.public && next.origin() != target.url.origin())
        {
            return Err(Error::Other(
                "download redirect left the permitted origin or transport".into(),
            ));
        }
        url = next;
    }
    Err(Error::Other("too many download redirects".into()))
}

fn response_error(status: StatusCode) -> Error {
    Error::Api {
        status: status.as_u16(),
        code: None,
        message: format!("download returned HTTP {status}"),
    }
}

fn probe(client: &NasClient, target: &DownloadTarget, dest: &Path) -> Result<Value> {
    let mut response = stream(client, target, Method::HEAD, None, None, Some(API_TIMEOUT))?;
    if matches!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED
    ) {
        response = stream(
            client,
            target,
            Method::GET,
            Some("bytes=0-0".into()),
            None,
            Some(API_TIMEOUT),
        )?;
    }
    if !response.status().is_success() {
        return Err(response_error(response.status()));
    }
    let headers = response.headers();
    Ok(json!({
        "dest": dest.to_string_lossy(),
        "status": response.status().as_u16(),
        "content_type": header_text(headers, CONTENT_TYPE),
        "content_length": header_text(headers, CONTENT_LENGTH),
        "content_range": header_text(headers, CONTENT_RANGE),
        "content_disposition": header_text(headers, CONTENT_DISPOSITION),
        "dry_run": true,
    }))
}

fn header_text(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<&str> {
    headers.get(name)?.to_str().ok()
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn partial_path(dest: &Path, source: &str) -> PathBuf {
    // The hash binds the part to both the local destination and the remote resource.
    // Signed URL tokens can change on every invocation and never enter the filename.
    let filename = dest.file_name().unwrap_or_default().as_encoded_bytes();
    let mut identity = filename.to_vec();
    identity.push(0);
    identity.extend_from_slice(source.as_bytes());
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, &identity);
    parent_dir(dest).join(format!(".unas-{id}.part"))
}

fn open_part(path: &Path) -> Result<Handle> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(path)?.file_type().is_file() {
                return Err(Error::Other(
                    "download part path is not a regular file".into(),
                ));
            }
            OpenOptions::new().read(true).write(true).open(path)?
        }
        Err(error) => return Err(error.into()),
    };
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(Error::Other(
                "another download is writing to this destination".into(),
            ));
        }
        Err(TryLockError::Error(error)) => return Err(error.into()),
    }
    let part = Handle::from_file(file)?;
    if !fs::symlink_metadata(path)?.file_type().is_file() || part != Handle::from_path(path)? {
        return Err(Error::Other(
            "download part was replaced while opening it; retry the download".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if part.as_file().metadata()?.nlink() != 1 {
            return Err(Error::Other(
                "download part is linked to another file".into(),
            ));
        }
    }
    Ok(part)
}

fn load_validator(path: &Path) -> Option<HeaderValue> {
    if !fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(1025)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 1024 || bytes.is_empty() {
        return None;
    }
    HeaderValue::from_bytes(&bytes).ok()
}

fn current_validator(headers: &HeaderMap) -> Option<&HeaderValue> {
    if let Some(etag) = headers.get(ETAG)
        && etag.as_bytes().starts_with(b"\"")
        && etag.as_bytes().ends_with(b"\"")
    {
        return Some(etag);
    }
    headers.get(LAST_MODIFIED)
}

fn remove_optional(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn store_validator(path: &Path, validator: Option<&HeaderValue>) -> Result<()> {
    if let Some(validator) = validator {
        let mut temporary = NamedTempFile::new_in(parent_dir(path))?;
        temporary.write_all(validator.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
    } else {
        remove_optional(path)?;
    }
    Ok(())
}

struct StreamLength {
    end: Option<u64>,
    total: Option<u64>,
}

fn stream_length(headers: &HeaderMap, partial: bool, offset: u64) -> Result<StreamLength> {
    let length = header_text(headers, CONTENT_LENGTH)
        .map(|length| length.parse::<u64>())
        .transpose()
        .map_err(|_| Error::Other("download returned an invalid Content-Length".into()))?;
    if !partial {
        return Ok(StreamLength {
            end: length,
            total: length,
        });
    }
    let invalid_range = || Error::Other("download returned an invalid Content-Range".into());
    let (span, total) = header_text(headers, CONTENT_RANGE)
        .and_then(|header| header.strip_prefix("bytes "))
        .and_then(|header| header.split_once('/'))
        .ok_or_else(invalid_range)?;
    let (start, end) = span.split_once('-').ok_or_else(invalid_range)?;
    let start = start.parse::<u64>().map_err(|_| invalid_range())?;
    let end = end
        .parse::<u64>()
        .ok()
        .and_then(|end| end.checked_add(1))
        .ok_or_else(invalid_range)?;
    let total = total.parse::<u64>().map_err(|_| invalid_range())?;
    if start != offset
        || start >= end
        || end > total
        || length.is_some_and(|length| length != end - start)
    {
        return Err(invalid_range());
    }
    Ok(StreamLength {
        end: Some(end),
        total: Some(total),
    })
}

fn persist_part(part: PathBuf, dest: &Path, overwrite: bool) -> Result<()> {
    let mut path = TempPath::try_from_path(part)?;
    path.disable_cleanup(true);
    let result = if overwrite {
        path.persist(dest)
    } else {
        path.persist_noclobber(dest)
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            Err(Error::Usage(format!(
                "destination already exists: {} (use --overwrite)",
                dest.display()
            )))
        }
        Err(error) => Err(error.error.into()),
    }
}

fn check_destination(dest: &Path, overwrite: bool) -> Result<()> {
    match fs::symlink_metadata(dest) {
        Ok(_) if !overwrite => Err(Error::Usage(format!(
            "destination already exists: {} (use --overwrite)",
            dest.display()
        ))),
        Ok(metadata) if metadata.is_dir() => Err(Error::Usage("destination is a directory".into())),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn receive(
    client: &NasClient,
    target: &DownloadTarget,
    source_identity: &str,
    source_name: Option<&str>,
    options: DownloadOptions<'_>,
) -> Result<Value> {
    let dest = target_path(options.dest, source_name)?;
    if options.dry_run {
        return probe(client, target, &dest);
    }
    check_destination(&dest, options.overwrite)?;
    fs::create_dir_all(parent_dir(&dest))?;
    let part = partial_path(&dest, source_identity);
    let validator_path = part.with_extension("validator");
    let mut locked_part = open_part(&part)?;
    let file = locked_part.as_file_mut();
    check_destination(&dest, options.overwrite)?;

    let validator = load_validator(&validator_path);
    let mut offset = if validator.is_some() {
        file.metadata()?.len()
    } else {
        0
    };
    let range = (offset > 0).then(|| format!("bytes={offset}-"));
    let mut response = stream(client, target, Method::GET, range, validator.as_ref(), None)?;
    if offset > 0 && response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        offset = 0;
        drop(response);
        response = stream(client, target, Method::GET, None, None, None)?;
    }

    let partial = match response.status() {
        StatusCode::OK => false,
        StatusCode::PARTIAL_CONTENT if offset > 0 => true,
        status => return Err(response_error(status)),
    };
    if header_text(response.headers(), CONTENT_ENCODING)
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(Error::Other(
            "download returned an unsupported Content-Encoding".into(),
        ));
    }
    let expected = stream_length(response.headers(), partial, offset)?;
    if partial {
        if current_validator(response.headers()).is_some_and(|new| Some(new) != validator.as_ref())
        {
            return Err(Error::Other(
                "download validator changed during a ranged response".into(),
            ));
        }
    } else {
        offset = 0;
        file.set_len(0)?;
        store_validator(&validator_path, current_validator(response.headers()))?;
    }
    file.seek(SeekFrom::Start(offset))?;

    let mut written = offset;
    let mut buffer = vec![0; DOWNLOAD_BUFFER];
    let mut indicator = Progress::new(options.progress);
    loop {
        let count = response
            .read(&mut buffer)
            .map_err(|_| Error::Network("download stream ended unexpectedly".into()))?;
        if count == 0 {
            break;
        }
        written += count as u64;
        if expected.end.is_some_and(|end| written > end) {
            return Err(Error::Other(
                "download stream exceeded the advertised length".into(),
            ));
        }
        file.write_all(&buffer[..count])?;
        indicator.report(written, expected.total, false);
    }
    if expected.end.is_some_and(|end| written != end)
        || expected.total.is_some_and(|total| written != total)
    {
        return Err(Error::Other(format!(
            "incomplete download: received {written} bytes"
        )));
    }
    file.sync_all()?;
    // Keep the part lock held through the move so another writer cannot modify the file
    // after it has become visible at the final path.
    let validator = load_validator(&validator_path);
    remove_optional(&validator_path)?;
    if let Err(error) = persist_part(part, &dest, options.overwrite) {
        let _ = store_validator(&validator_path, validator.as_ref());
        return Err(error);
    }
    drop(locked_part);
    indicator.report(written, expected.total, true);
    Ok(json!({
        "dest": dest.to_string_lossy(),
        "bytes": written,
        "resumed_from": offset,
        "dry_run": false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use rand::{SeedableRng, rngs::StdRng};
    use rsa::{RsaPrivateKey, pkcs8::EncodePublicKey};
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};

    fn fake_downloads(
        responses: &'static [&'static str],
    ) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/download", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            responses
                .iter()
                .map(|response| {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let mut stream = accept(&listener, deadline);
                    let (request, _) = read_request(&mut stream);
                    stream.write_all(response.as_bytes()).unwrap();
                    request.to_ascii_lowercase()
                })
                .collect()
        });
        (url, server)
    }

    fn accept(listener: &TcpListener, deadline: Instant) -> TcpStream {
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("mock transfer server failed to accept connection: {error}"),
            }
        }
    }

    fn read_request(stream: &mut TcpStream) -> (String, Vec<u8>) {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" {
                break;
            }
            headers.push_str(&line);
        }
        let length = headers
            .lines()
            .find_map(|header| {
                let (name, value) = header.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        (headers, body)
    }

    #[test]
    fn upload_uses_the_offset_confirmed_by_the_nas() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let key = RsaPrivateKey::new(&mut StdRng::seed_from_u64(42), 1024).unwrap();
            let public_key = key.to_public_key().to_public_key_der().unwrap();
            let mut upload_id = String::new();
            for step in 0..6 {
                let mut stream = accept(&listener, Instant::now() + Duration::from_secs(5));
                let (headers, request_body) = read_request(&mut stream);
                let answer = match step {
                    0 => {
                        assert!(headers.starts_with("POST /ugreen/v1/verify/check "));
                        json!({"code": 200, "xRsaToken": STANDARD.encode(public_key.as_bytes())})
                    }
                    1 => {
                        assert!(headers.starts_with("POST /ugreen/v1/verify/login "));
                        json!({"code": 200, "data": {"token": "test-token"}})
                    }
                    2 => {
                        assert!(headers.starts_with("POST /ugreen/v1/filemgr/pathExist?"));
                        json!({"code": 200, "data": {"result": true}})
                    }
                    3 => {
                        assert!(headers.starts_with("POST /ugreen/v1/filemgr/fileUpload?"));
                        let body = String::from_utf8(request_body).unwrap();
                        assert!(body.contains("filename=\"%E6%B5%8B%20%E8%AF%95.txt\""));
                        assert!(body.contains("name=\"first_request\"\r\n\r\ntrue\r\n"));
                        upload_id = body
                            .split_once("name=\"uuid\"\r\n\r\n")
                            .unwrap()
                            .1
                            .lines()
                            .next()
                            .unwrap()
                            .to_owned();
                        json!({"code": 200, "data": {"result": false}})
                    }
                    4 => {
                        assert!(headers.starts_with("GET /ugreen/v1/filemgr/getUpdateTmpInfo?"));
                        assert!(headers.contains(&format!("uuid={upload_id}")));
                        json!({"code": 200, "data": {"result": "3"}})
                    }
                    _ => {
                        assert!(headers.starts_with("POST /ugreen/v1/filemgr/fileUploadV2?"));
                        let header = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("ug-param: "))
                            .unwrap();
                        let parameters: Value = serde_json::from_str(header).unwrap();
                        assert_eq!(parameters["uuid"], upload_id);
                        assert_eq!(parameters["dir"], "%2Fvolume1%2Fshare");
                        assert_eq!(parameters["file_name"], "测 试.txt");
                        assert_eq!(parameters["begin_size"], 3);
                        assert_eq!(parameters["current_size"], 3);
                        assert_eq!(parameters["action_type"], 0);
                        assert_eq!(request_body, b"def");
                        json!({"code": 200, "data": {"result": "/volume1/share/测 试.txt"}})
                    }
                }
                .to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len()).unwrap();
            }
        });
        let mut client = NasClient::new(&origin, false, None).unwrap();
        client.connect("user", Some("password")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        fs::write(&source, b"abcdef").unwrap();
        let uploaded = upload(
            &mut client,
            &source,
            "/volume1/share",
            Some("测 试.txt"),
            false,
            false,
        )
        .unwrap();
        assert_eq!(uploaded["path"], "/volume1/share/测 试.txt");
        worker.join().unwrap();
    }

    #[test]
    fn resumes_the_same_share_with_a_fresh_signed_url() {
        let (url, server) = fake_downloads(&[
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nETag: \"original\"\r\nConnection: close\r\n\r\nabc",
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nETag: \"original\"\r\nConnection: close\r\n\r\ndef",
        ]);
        let client = NasClient::new(&url, false, None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file");
        let options = DownloadOptions {
            dest: &dest,
            dry_run: false,
            overwrite: true,
            progress: false,
        };
        fs::write(&dest, b"previous file").unwrap();

        let signed = format!("{url}?task_id=first");
        assert!(download_public(&client, &signed, "share/file", None, options).is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"previous file");

        let signed = format!("{url}?task_id=second");
        let result = download_public(&client, &signed, "share/file", None, options).unwrap();
        assert_eq!(result["resumed_from"], 3);
        assert_eq!(fs::read(&dest).unwrap(), b"abcdef");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        let requests = server.join().unwrap();
        assert!(!requests[0].contains("\r\nrange:"));
        assert!(requests[1].contains("\r\nrange: bytes=3-\r\n"));
        assert!(requests[1].contains("\r\nif-range: \"original\"\r\n"));
    }

    #[test]
    fn keeps_parts_separate_for_distinct_share_files() {
        let (url, server) = fake_downloads(&[
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nETag: \"first\"\r\nConnection: close\r\n\r\nabc",
            "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nnew",
        ]);
        let client = NasClient::new(&url, false, None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file");
        let options = DownloadOptions {
            dest: &dest,
            dry_run: false,
            overwrite: false,
            progress: false,
        };
        assert!(download_public(&client, &url, "share/first", None, options).is_err());
        let result = download_public(&client, &url, "share/second", None, options).unwrap();
        assert_eq!(result["resumed_from"], 0);
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        assert!(!server.join().unwrap()[1].contains("\r\nrange:"));
    }

    #[test]
    fn invalid_ranged_responses_are_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 3-5/6"));
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("3"));
        assert_eq!(stream_length(&headers, true, 3).unwrap().total, Some(6));
        assert!(stream_length(&headers, true, 2).is_err());
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("4"));
        assert!(stream_length(&headers, true, 3).is_err());
    }

    #[test]
    fn publish_preserves_a_file_created_during_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join(".part");
        let dest = dir.path().join("file");
        fs::write(&part, b"download").unwrap();
        fs::write(&dest, b"existing").unwrap();

        assert!(persist_part(part.clone(), &dest, false).is_err());
        assert_eq!(fs::read(&part).unwrap(), b"download");
        assert_eq!(fs::read(&dest).unwrap(), b"existing");
        persist_part(part.clone(), &dest, true).unwrap();
        assert!(!part.exists());
        assert_eq!(fs::read(&dest).unwrap(), b"download");
    }
}
