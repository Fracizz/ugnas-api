use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::HeaderValue;
use reqwest::{Method, Url};
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use serde_json::{Value, json};
use uuid::Uuid;

const API_TIMEOUT: Duration = Duration::from_secs(20);
const TRANSFER_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub(crate) enum Error {
    Usage(String),
    Auth(String),
    Network(String),
    Api {
        status: u16,
        code: Option<i64>,
        message: String,
    },
    Io(io::Error),
    Other(String),
    Timeout(String),
}

impl Error {
    pub(crate) fn status(&self) -> Option<u16> {
        match self {
            Self::Api { status, .. } => Some(*status),
            _ => None,
        }
    }

    fn session_rejected(&self) -> bool {
        matches!(
            self,
            Self::Api { status: 401, .. }
                | Self::Api {
                    code: Some(1024),
                    ..
                }
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message)
            | Self::Auth(message)
            | Self::Network(message)
            | Self::Other(message)
            | Self::Timeout(message) => formatter.write_str(message),
            Self::Api {
                status,
                code,
                message,
            } => {
                write!(formatter, "{message} (HTTP {status}")?;
                if let Some(code) = code {
                    write!(formatter, ", UGOS code {code}")?;
                }
                formatter.write_str(")")
            }
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        let message = if error.is_timeout() {
            "request timed out"
        } else if error.is_connect() {
            "could not connect to the server or validate its TLS certificate"
        } else if error.is_builder() {
            "could not build the HTTP request"
        } else {
            "HTTP request or response stream failed"
        };
        Self::Network(message.into())
    }
}

pub(crate) struct NasClient {
    base_url: Url,
    http: Client,
    session_path: Option<PathBuf>,
    client_id: HeaderValue,
    username: String,
    password: Option<String>,
    token: Option<String>,
    auth_type: String,
    login_data: Value,
    reused_session: bool,
}

impl NasClient {
    pub(crate) fn new(
        base_url: &str,
        insecure: bool,
        session_path: Option<PathBuf>,
    ) -> Result<Self> {
        let base_url = parse_base_url(base_url)?;
        let http = Client::builder()
            .timeout(TRANSFER_IDLE_TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(insecure)
            .build()?;
        let client_id = HeaderValue::from_str(&Uuid::new_v4().to_string())
            .map_err(|_| Error::Other("could not generate a client ID".into()))?;
        Ok(Self {
            base_url,
            http,
            session_path,
            client_id,
            username: String::new(),
            password: None,
            token: None,
            auth_type: "url".into(),
            login_data: Value::Null,
            reused_session: false,
        })
    }

    pub(crate) fn connect(&mut self, username: &str, password: Option<&str>) -> Result<()> {
        if username.trim().is_empty() {
            return Err(Error::Usage("NAS username is required".into()));
        }
        self.username = username.into();
        self.password = password
            .filter(|password| !password.is_empty())
            .map(str::to_owned);
        self.clear_session();

        if let Some(saved) = self.load_session() {
            self.apply_session(saved)?;
            if self.session_is_current() && self.validate_session()? {
                self.reused_session = true;
                return Ok(());
            }
            match self.refresh() {
                Ok(()) => return Ok(()),
                Err(error @ (Error::Network(_) | Error::Io(_))) => return Err(error),
                Err(_) => self.clear_session(),
            }
        }

        self.login()
    }

    pub(crate) fn http(&self) -> &Client {
        &self.http
    }

    pub(crate) fn url(&self, path: &str) -> Result<Url> {
        let url = self
            .base_url
            .join(path.trim_start_matches('/'))
            .map_err(|_| Error::Usage("invalid NAS endpoint".into()))?;
        if url.origin() != self.base_url.origin()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::Usage(
                "NAS endpoint must use the configured origin".into(),
            ));
        }
        Ok(url)
    }

    pub(crate) fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub(crate) fn client_id(&self) -> &str {
        self.client_id.to_str().expect("client ID is ASCII")
    }

    pub(crate) fn token_in_url(&self) -> bool {
        self.auth_type == "url"
    }

    pub(crate) fn login_data(&self) -> &Value {
        &self.login_data
    }

    pub(crate) fn reused_session(&self) -> bool {
        self.reused_session
    }

    pub(crate) fn request(
        &mut self,
        method: Method,
        path: &str,
        build: impl Fn(RequestBuilder) -> RequestBuilder,
    ) -> Result<Value> {
        let first = self.send_json(method.clone(), path, true, &build);
        match first {
            Err(error) if error.session_rejected() => {
                self.recover()?;
                self.send_json(method, path, true, build)
                    .map_err(auth_error)
            }
            result => result,
        }
    }

    fn send_json(
        &self,
        method: Method,
        path: &str,
        authenticated: bool,
        build: impl FnOnce(RequestBuilder) -> RequestBuilder,
    ) -> Result<Value> {
        let request = self.request_builder(method, path, authenticated)?;
        check_response(build(request).send()?)
    }

    fn request_builder(
        &self,
        method: Method,
        path: &str,
        authenticated: bool,
    ) -> Result<RequestBuilder> {
        let mut request = self
            .http
            .request(method, self.url(path)?)
            .timeout(API_TIMEOUT)
            .header("UG-Agent", "PC/WEB")
            .header("Accept", "application/json, text/plain, */*")
            .header("X-Specify-Language", "zh-CN")
            .header("Client-Id", self.client_id.clone())
            .header("Client-Version", "1.18.0");
        if authenticated {
            let token = self
                .token()
                .ok_or_else(|| Error::Auth("NAS login required".into()))?;
            request = request.query(&[("token", token)]);
            if !self.token_in_url() {
                let mut header = HeaderValue::from_str(token)
                    .map_err(|_| Error::Auth("invalid NAS session token".into()))?;
                header.set_sensitive(true);
                request = request.header("X-Ugreen-Token", header);
            }
        }
        Ok(request)
    }

    fn password_key(&self) -> Result<RsaPublicKey> {
        let response = self
            .request_builder(Method::POST, "/ugreen/v1/verify/check", false)?
            .json(&json!({ "username": self.username }))
            .send()?;
        let header = response
            .headers()
            .get("x-rsa-token")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let payload = check_response(response).map_err(auth_error)?;
        let public_key = header
            .as_deref()
            .filter(|key| !key.trim().is_empty())
            .or_else(|| payload["xRsaToken"].as_str())
            .ok_or_else(|| Error::Auth("NAS did not return the RSA login key".into()))?;
        decode_rsa_key(public_key)
    }

    fn login(&mut self) -> Result<()> {
        let password = self
            .password
            .as_deref()
            .ok_or_else(|| Error::Auth("no reusable session; provide a NAS password".into()))?;
        let encrypted = self
            .password_key()?
            .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, password.as_bytes())
            .map_err(|_| Error::Auth("could not encrypt the NAS password".into()))?;
        let payload = self
            .send_json(Method::POST, "/ugreen/v1/verify/login", false, |request| {
                request
                    .header("UG-Client-Id", self.client_id.clone())
                    .json(&json!({
                        "username": self.username,
                        "password": STANDARD.encode(encrypted),
                        "keepalive": true,
                        "otp": false,
                        "is_simple": false,
                    }))
            })
            .map_err(|error| match error {
                Error::Api { .. } => Error::Auth(format!("NAS login failed: {error}")),
                error => error,
            })?;
        self.set_login_data(payload["data"].clone())?;
        self.reused_session = false;
        self.save_session()
    }

    fn refresh(&mut self) -> Result<()> {
        let payload =
            self.send_json(Method::PUT, "/ugreen/v1/verify/refresh", true, |request| {
                request
            })?;
        let response = payload["data"]
            .as_object()
            .filter(|data| {
                data.get("token")
                    .and_then(Value::as_str)
                    .is_some_and(|token| !token.is_empty())
            })
            .ok_or_else(|| Error::Auth("NAS refresh did not return a new session token".into()))?;
        let mut merged = self.login_data.as_object().cloned().unwrap_or_default();
        for key in EXPIRY_KEYS {
            merged.remove(*key);
        }
        merged.extend(response.clone());
        self.set_login_data(Value::Object(merged))?;
        self.reused_session = true;
        self.save_session()
    }

    fn recover(&mut self) -> Result<()> {
        match self.refresh() {
            Ok(()) => Ok(()),
            Err(error @ (Error::Network(_) | Error::Io(_))) => Err(error),
            Err(_) => self.login(),
        }
    }

    fn set_login_data(&mut self, mut data: Value) -> Result<()> {
        let token = data["token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .ok_or_else(|| Error::Auth("NAS login response missing a session token".into()))?;
        self.token = Some(token.into());
        self.auth_type = data["auth_type"]
            .as_str()
            .filter(|auth_type| !auth_type.is_empty())
            .unwrap_or("url")
            .into();
        if let Some(expiration) = expiration_time(&data) {
            data["expires_at"] = json!(expiration);
        }
        self.login_data = data;
        Ok(())
    }

    fn clear_session(&mut self) {
        self.token = None;
        self.auth_type = "url".into();
        self.login_data = Value::Null;
        self.reused_session = false;
    }

    fn validate_session(&self) -> Result<bool> {
        match self.send_json(
            Method::GET,
            "/ugreen/v1/filemgr/getVolumes",
            true,
            |request| request,
        ) {
            Ok(_) => Ok(true),
            Err(error) if error.session_rejected() => Ok(false),
            Err(error @ Error::Network(_)) => Err(error),
            Err(_) => match self.send_json(
                Method::GET,
                "/ugreen/v1/filemgr/shareList",
                true,
                |request| request,
            ) {
                Ok(_) => Ok(true),
                Err(error @ Error::Network(_)) => Err(error),
                Err(_) => Ok(false),
            },
        }
    }

    fn session_is_current(&self) -> bool {
        expiration_time(&self.login_data).is_none_or(|expiration| expiration > unix_time())
    }

    fn load_session(&self) -> Option<Value> {
        let path = self.session_path.as_deref()?;
        let body = fs::read(path).ok()?;
        let mut saved: Value = serde_json::from_slice(&body).ok()?;
        if saved["username"].as_str() != Some(self.username.as_str()) {
            return None;
        }
        let base_url = parse_base_url(saved["base_url"].as_str()?).ok()?;
        if self.base_url != base_url {
            return None;
        }
        let token = decode_session_token(saved["token"].as_str()?).ok()?;
        if token.is_empty() {
            return None;
        }
        saved["token"] = Value::String(token);
        Some(saved)
    }

    fn apply_session(&mut self, saved: Value) -> Result<()> {
        if let Some(client_id) = saved["client_id"]
            .as_str()
            .and_then(|value| HeaderValue::from_str(value).ok())
            .filter(|value| !value.is_empty() && value.to_str().is_ok())
        {
            self.client_id = client_id;
        }
        self.set_login_data(saved)
    }

    fn save_session(&self) -> Result<()> {
        let Some(path) = self.session_path.as_deref() else {
            return Ok(());
        };
        let Some(token) = self.token() else {
            return Err(Error::Auth("NAS login required".into()));
        };
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        let session = json!({
            "username": self.username,
            "base_url": self.base_url.as_str().trim_end_matches('/'),
            "token": encode_session_token(token)?,
            "auth_type": self.auth_type,
            "client_id": self.client_id.to_str().ok(),
            "uglink": self.login_data["uglink"],
            "http_port": self.login_data["http_port"],
            "https_port": self.login_data["https_port"],
            "expires_at": expiration_time(&self.login_data),
        });
        serde_json::to_writer_pretty(&mut temporary, &session)
            .map_err(|error| Error::Other(format!("could not write the NAS session: {error}")))?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    }
}

fn parse_base_url(raw: &str) -> Result<Url> {
    let mut url =
        Url::parse(raw.trim()).map_err(|_| Error::Usage("invalid NAS base URL".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Usage(
            "NAS base URL must be HTTP(S) without credentials, query, or fragment".into(),
        ));
    }
    let directory = format!("{}/", url.path().trim_end_matches('/'));
    url.set_path(&directory);
    Ok(url)
}

fn decode_rsa_key(raw: &str) -> Result<RsaPublicKey> {
    let raw = raw.trim();
    if raw.starts_with("-----BEGIN") {
        return parse_rsa_pem(raw);
    }
    let compact: String = raw.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = STANDARD
        .decode(&compact)
        .or_else(|_| STANDARD_NO_PAD.decode(&compact))
        .map_err(|_| Error::Auth("invalid base64 in NAS RSA login key".into()))?;
    if bytes.starts_with(b"-----BEGIN") {
        let pem = std::str::from_utf8(&bytes)
            .map_err(|_| Error::Auth("invalid PEM encoding in NAS RSA login key".into()))?;
        parse_rsa_pem(pem)
    } else {
        RsaPublicKey::from_public_key_der(&bytes)
            .or_else(|_| RsaPublicKey::from_pkcs1_der(&bytes))
            .map_err(|_| Error::Auth("invalid NAS RSA login key".into()))
    }
}

fn parse_rsa_pem(pem: &str) -> Result<RsaPublicKey> {
    RsaPublicKey::from_public_key_pem(pem)
        .or_else(|_| RsaPublicKey::from_pkcs1_pem(pem))
        .map_err(|_| Error::Auth("invalid NAS RSA login key".into()))
}

fn auth_error(error: Error) -> Error {
    if error.session_rejected() {
        Error::Auth("NAS rejected the session".into())
    } else {
        error
    }
}

pub(crate) fn check_response(response: Response) -> Result<Value> {
    let status = response.status();
    let payload: Value = response.json().map_err(|error| {
        if !status.is_success() || error.is_decode() {
            Error::Api {
                status: status.as_u16(),
                code: None,
                message: if status.is_success() {
                    "NAS returned invalid JSON".into()
                } else {
                    "NAS returned an unsuccessful response".into()
                },
            }
        } else {
            error.into()
        }
    })?;
    let code = integer(&payload["code"]);
    if !status.is_success() || !matches!(code, Some(200 | 1000 | 1013 | 1118)) {
        return Err(Error::Api {
            status: status.as_u16(),
            code,
            message: "NAS returned an unsuccessful response".into(),
        });
    }
    Ok(payload)
}

pub(crate) fn data(payload: &Value) -> &Value {
    payload
        .get("data")
        .filter(|value| !value.is_null())
        .unwrap_or(payload)
}

fn integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

const EXPIRY_KEYS: &[&str] = &[
    "expires_at",
    "expire_time",
    "expired_at",
    "token_expire",
    "expire",
    "expires_in",
];

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn expiration_time(data: &Value) -> Option<i64> {
    for key in &EXPIRY_KEYS[..EXPIRY_KEYS.len() - 1] {
        if let Some(raw) = integer(&data[key]) {
            let seconds = if raw > 10_000_000_000 {
                raw / 1000
            } else {
                raw
            };
            if seconds > 1_000_000_000 {
                return Some(seconds);
            }
        }
    }
    unix_time().checked_add(integer(&data["expires_in"])?)
}

#[cfg(not(windows))]
fn encode_session_token(token: &str) -> Result<String> {
    Ok(token.to_owned())
}

#[cfg(not(windows))]
fn decode_session_token(token: &str) -> Result<String> {
    if token.starts_with("dpapi:") {
        return Err(Error::Auth(
            "Windows session token cannot be read on this platform".into(),
        ));
    }
    Ok(token.to_owned())
}

#[cfg(windows)]
fn encode_session_token(token: &str) -> Result<String> {
    let protected = dpapi(token.as_bytes(), true)?;
    Ok(format!("dpapi:{}", STANDARD.encode(protected)))
}

#[cfg(windows)]
fn decode_session_token(token: &str) -> Result<String> {
    let protected = token
        .strip_prefix("dpapi:")
        .and_then(|encoded| STANDARD.decode(encoded).ok())
        .ok_or_else(|| Error::Auth("invalid encrypted NAS session token".into()))?;
    String::from_utf8(dpapi(&protected, false)?)
        .map_err(|_| Error::Auth("invalid encrypted NAS session token".into()))
}

#[cfg(windows)]
fn dpapi(input: &[u8], protect: bool) -> Result<Vec<u8>> {
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    let source = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(input.len())
            .map_err(|_| Error::Auth("NAS session token is too long".into()))?,
        pbData: input.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // DPAPI reads the input blob and assigns a LocalFree-owned buffer to output on success.
    let success = unsafe {
        if protect {
            CryptProtectData(
                &source,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &source,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if success == 0 {
        return Err(io::Error::last_os_error().into());
    }
    if output.pbData.is_null() {
        return Err(Error::Auth(
            "Windows could not read the NAS session token".into(),
        ));
    }
    // A successful DPAPI call supplies a valid buffer of cbData bytes.
    let bytes = unsafe {
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());
        bytes
    };
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{SeedableRng, rngs::StdRng};
    use rsa::{RsaPrivateKey, pkcs8::EncodePublicKey};
    use std::io::{BufRead, BufReader, Read};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn read_request(reader: &mut BufReader<&mut TcpStream>) -> (String, String, Vec<u8>) {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let mut headers = String::new();
        let mut length = 0;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            if header.trim().is_empty() {
                break;
            }
            header.make_ascii_lowercase();
            if let Some(value) = header.strip_prefix("content-length:") {
                length = value.trim().parse().unwrap();
            }
            headers.push_str(&header);
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        (line, headers, body)
    }

    #[test]
    fn login_reuses_saved_session_and_replays_after_refresh() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let key = RsaPrivateKey::new(&mut StdRng::seed_from_u64(42), 1024).unwrap();
            let public_key = key.to_public_key().to_public_key_der().unwrap();
            for expected in 0..6 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let (line, headers, body) = read_request(&mut BufReader::new(&mut stream));
                let mut extra_headers = String::new();
                let answer = match expected {
                    0 => {
                        assert!(line.starts_with("POST /ugreen/v1/verify/check "));
                        assert_eq!(
                            serde_json::from_slice::<Value>(&body).unwrap(),
                            json!({"username": "test"})
                        );
                        extra_headers = format!(
                            "X-Rsa-Token: {}\r\n",
                            STANDARD.encode(public_key.as_bytes())
                        );
                        json!({"code": 200})
                    }
                    1 => {
                        assert!(line.starts_with("POST /ugreen/v1/verify/login "));
                        assert!(headers.contains("ug-client-id:"));
                        let body: Value = serde_json::from_slice(&body).unwrap();
                        let ciphertext =
                            STANDARD.decode(body["password"].as_str().unwrap()).unwrap();
                        assert_eq!(
                            key.decrypt(Pkcs1v15Encrypt, &ciphertext).unwrap(),
                            b"dummy pass"
                        );
                        json!({"code": 200, "data": {"token": "first-token", "auth_type": "header"}})
                    }
                    2 => {
                        assert!(
                            line.starts_with(
                                "GET /ugreen/v1/filemgr/getVolumes?token=first-token "
                            )
                        );
                        assert!(headers.contains("x-ugreen-token: first-token"));
                        json!({"code": 200})
                    }
                    3 | 5 => {
                        let token = if expected == 3 {
                            "first-token"
                        } else {
                            "second-token"
                        };
                        assert!(line.starts_with(&format!("POST /example?token={token} ")));
                        assert!(headers.contains(&format!("x-ugreen-token: {token}")));
                        assert_eq!(
                            serde_json::from_slice::<Value>(&body).unwrap(),
                            json!({"path": "/share/file"})
                        );
                        json!({"code": if expected == 3 { 1024 } else { 200 }})
                    }
                    4 => {
                        assert!(
                            line.starts_with("PUT /ugreen/v1/verify/refresh?token=first-token ")
                        );
                        json!({"code": 200, "data": {"token": "second-token"}})
                    }
                    _ => unreachable!(),
                };
                let answer = answer.to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{answer}",
                    answer.len()
                ).unwrap();
            }
        });

        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("session.json");
        let mut client = NasClient::new(&base, false, Some(path.clone())).unwrap();
        client.connect("test", Some("dummy pass")).unwrap();
        assert!(!client.reused_session());
        let client_id = client.client_id().to_owned();
        drop(client);

        let mut client = NasClient::new(&base, false, Some(path.clone())).unwrap();
        client.connect("test", None).unwrap();
        assert!(client.reused_session());
        assert_eq!(client_id, client.client_id());
        client
            .request(Method::POST, "/example", |request| {
                request.json(&json!({"path": "/share/file"}))
            })
            .unwrap();
        assert_eq!(client.token(), Some("second-token"));
        worker.join().unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
        }
        #[cfg(windows)]
        assert!(!fs::read_to_string(path).unwrap().contains("second-token"));
    }

    #[test]
    fn nas_base_and_endpoints_reject_credentials_and_foreign_origins() {
        assert!(parse_base_url("https://user:secret@example.com").is_err());
        assert!(parse_base_url("https://example.com?token=secret").is_err());
        let client = NasClient::new("https://example.com/nas//", false, None).unwrap();
        assert_eq!(
            client
                .url("/ugreen/v1/filemgr/getVolumes")
                .unwrap()
                .as_str(),
            "https://example.com/nas/ugreen/v1/filemgr/getVolumes"
        );
        assert!(client.url("https://foreign.example/path").is_err());
    }
}
