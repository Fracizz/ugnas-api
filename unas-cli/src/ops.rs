use std::thread;
use std::time::{Duration, Instant};

use reqwest::header::{ORIGIN, REFERER};
use reqwest::{Method, Url};
use serde_json::{Value, json};

use crate::client::{Error, NasClient, Result, check_response, data};

const FILE_TIMEOUT: Duration = Duration::from_secs(20);
const EMPTY: &[Value] = &[];

pub(crate) fn roots(client: &mut NasClient) -> Result<Value> {
    let payload = client.request(Method::GET, "/ugreen/v2/filemgr/getRootDirs", |request| {
        request
    })?;
    Ok(match items(&payload, 0) {
        Some(roots) => json!({"items": roots}),
        None => json!({"items": [], "data": data(&payload)}),
    })
}

pub(crate) fn list(
    client: &mut NasClient,
    path: &str,
    page: u32,
    limit: u32,
    all: bool,
) -> Result<Value> {
    let path = nas_path(path)?;
    let entries = collect_pages(page, limit, all, |page| {
        let v2 = json!({
            "path": path, "page": page, "limit": limit, "data_type": 0, "show_dir": true,
            "left_no_page_show": false, "left_count": 5500, "is_shield_recycle": false,
        });
        match client.request(
            Method::POST,
            "/ugreen/v2/filemgr/getDirFileListV2",
            |request| request.json(&v2),
        ) {
            Err(error) if matches!(error.status(), Some(404 | 405)) => {
                let v1 = json!({
                    "path": path, "page": page, "limit": limit, "sort_type": "name",
                    "reverse": false, "only_dir": false,
                });
                client.request(
                    Method::POST,
                    "/ugreen/v1/filemgr/getDirFileList",
                    |request| request.json(&v1),
                )
            }
            result => result,
        }
    })?;
    Ok(json!({"path": path, "page": page, "limit": limit, "items": entries}))
}

pub(crate) fn exists(client: &mut NasClient, path: &str) -> Result<bool> {
    path_exists(client, nas_path(path)?, FILE_TIMEOUT)
}

fn path_exists(client: &mut NasClient, path: &str, timeout: Duration) -> Result<bool> {
    let payload = client.request(Method::POST, "/ugreen/v1/filemgr/pathExist", |request| {
        request.timeout(timeout).json(&json!({"path": path}))
    })?;
    let value = &payload["data"];
    let result = value.get("result").unwrap_or(value);
    match result {
        Value::Bool(exists) => Ok(*exists),
        Value::Number(exists) => Ok(exists.as_f64() != Some(0.0)),
        Value::String(exists) if exists == "true" || exists == "1" => Ok(true),
        Value::String(exists) if exists == "false" || exists == "0" || exists.is_empty() => {
            Ok(false)
        }
        Value::Null => Ok(false),
        _ => Err(Error::Other("unexpected response from pathExist".into())),
    }
}

pub(crate) fn mkdir(client: &mut NasClient, path: &str) -> Result<()> {
    let deadline = Deadline::new(FILE_TIMEOUT, "NAS directory creation did not complete")?;
    ensure_dir(client, path, &deadline)
}

fn ensure_dir(client: &mut NasClient, path: &str, deadline: &Deadline) -> Result<()> {
    let path = nas_path(path)?;
    if path == "/" || path_exists(client, path, deadline.remaining()?.min(FILE_TIMEOUT))? {
        return Ok(());
    }
    if !path[1..].contains('/') {
        return Err(Error::Usage(
            "directory must be under an existing NAS root".into(),
        ));
    }
    let request_timeout = deadline.remaining()?.min(FILE_TIMEOUT);
    match client.request(Method::POST, "/ugreen/v2/filemgr/createFolder", |request| {
        request
            .timeout(request_timeout)
            .json(&json!({"path": path, "no_recursive_creation": false}))
    }) {
        Ok(_) => Ok(()),
        Err(
            error @ Error::Api {
                code: Some(1327), ..
            },
        ) => {
            if path_exists(client, path, deadline.remaining()?.min(FILE_TIMEOUT))? {
                Ok(())
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn delete(client: &mut NasClient, paths: &[String]) -> Result<Value> {
    validate_paths(paths)?;
    client.request(Method::POST, "/ugreen/v1/filemgr/delPaths", |request| {
        request.json(&json!({"paths": paths}))
    })
}

pub(crate) fn rename(client: &mut NasClient, path: &str, name: &str) -> Result<Value> {
    let path = non_root_path(path)?;
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\', '\0']) {
        return Err(Error::Usage("name must be a single NAS filename".into()));
    }
    let payload = client.request(Method::POST, "/ugreen/v2/filemgr/rename", |request| {
        request.json(&json!({"path": path, "new_name": name}))
    })?;
    Ok(data(&payload).clone())
}

pub(crate) fn copy_or_move(
    client: &mut NasClient,
    paths: &[String],
    dest: &str,
    move_files: bool,
    force: bool,
    timeout: Duration,
) -> Result<Value> {
    validate_paths(paths)?;
    let dest = nas_path(dest)?;
    let deadline = Deadline::new(timeout, "copy or move did not complete")?;
    let targets: Vec<_> = paths
        .iter()
        .map(|path| format!("{}/{}", dest.trim_end_matches('/'), basename(path)))
        .collect();
    ensure_dir(client, dest, &deadline)?;
    let mut target_already_exists = false;
    for target in &targets {
        if path_exists(client, target, deadline.remaining()?.min(FILE_TIMEOUT))? {
            target_already_exists = true;
            break;
        }
    }
    let body = json!({
        "dst": dest, "src": paths, "action_type": i32::from(force), "type": move_files,
        "intranet_share_id": 0,
    });
    let request_timeout = deadline.remaining()?.min(Duration::from_secs(180));
    let payload = client.request(Method::POST, "/ugreen/v2/filemgr/cpOrMvPath", |request| {
        request.timeout(request_timeout).json(&body)
    })?;
    if target_already_exists {
        // An existing destination does not tell us whether UGOS processed the new task.
        return Ok(json!({"status": "submitted", "paths": targets, "task": data(&payload)}));
    }
    loop {
        let mut appears = true;
        for target in &targets {
            if !path_exists(client, target, deadline.remaining()?.min(FILE_TIMEOUT))? {
                appears = false;
                break;
            }
        }
        if appears {
            return Ok(
                json!({"status": "targets_present", "paths": targets, "task": data(&payload)}),
            );
        }
        deadline.pause(Duration::from_millis(500))?;
    }
}

pub(crate) fn space(client: &mut NasClient, path: Option<&str>) -> Result<Value> {
    if let Some(path) = path {
        let path = nas_path(path)?;
        return match client.request(
            Method::POST,
            "/ugreen/v1/filemgr/getPathFreeSize",
            |request| request.json(&json!({"path": path})),
        ) {
            Ok(payload) => {
                let value = data(&payload);
                let free = unsigned(value.get("result").unwrap_or(value)).ok_or_else(|| {
                    Error::Other("getPathFreeSize did not return free bytes".into())
                })?;
                Ok(json!({"path": path, "free": free}))
            }
            Err(error @ Error::Api { .. }) => {
                let volumes = volumes(client)?;
                match volumes
                    .iter()
                    .filter_map(|volume| {
                        let root = volume["path"].as_str()?;
                        let free = unsigned(&volume["free"])?;
                        let matches = !root.is_empty()
                            && (path == root.trim_end_matches('/')
                                || path
                                    .strip_prefix(root.trim_end_matches('/'))
                                    .is_some_and(|rest| rest.starts_with('/')));
                        matches.then_some((root, free))
                    })
                    .max_by_key(|(root, _)| root.len())
                {
                    Some((volume, free)) => {
                        Ok(json!({"path": path, "volume": volume, "free": free}))
                    }
                    None => Err(error),
                }
            }
            Err(error) => Err(error),
        };
    }
    let volumes = volumes(client)?;
    let quota = client.request(Method::GET, "/ugreen/v2/user/directory/quota", |request| {
        request
    })?;
    Ok(json!({"volumes": volumes, "quota": data(&quota)}))
}

fn volumes(client: &mut NasClient) -> Result<Vec<Value>> {
    let payload = client.request(Method::GET, "/ugreen/v1/filemgr/getVolumes", |request| {
        request
    })?;
    Ok(items(&payload, 0)
        .unwrap_or(EMPTY)
        .iter()
        .filter(|item| item.is_object())
        .map(|item| {
            json!({
                "name": item["name"], "path": item["path"], "fs_type": item["fs_type"],
                "free": item["free"], "used": item["used"],
                "total": item.get("all").filter(|value| !value.is_null()).unwrap_or(&item["total"]),
            })
        })
        .collect())
}

pub(crate) fn size(client: &mut NasClient, path: &str, timeout: Duration) -> Result<Value> {
    let path = nas_path(path)?;
    let deadline = Deadline::new(timeout, "NAS size calculation did not complete")?;
    loop {
        let request_timeout = deadline.remaining()?.min(FILE_TIMEOUT);
        let payload = client.request(
            Method::GET,
            "/ugreen/v2/filemgr/getSizeAndCount",
            |request| {
                request
                    .timeout(request_timeout)
                    .query(&[("path", path), ("intranet_share_id", "0")])
            },
        )?;
        let result = data(&payload);
        if unsigned(&result["status"]) == Some(1) {
            return Ok(json!({
                "path": path, "size": result["size"], "takes_space": result["takes_space"],
                "file_count": result["file_count"], "dir_count": result["dir_count"],
                "hide_count": result["hide_count"], "status": result["status"],
            }));
        }
        deadline.pause(Duration::from_millis(400))?;
    }
}

pub(crate) fn pull(
    client: &mut NasClient,
    url: &str,
    dest: &str,
    force: bool,
    wait: bool,
    timeout: Duration,
) -> Result<Value> {
    validate_http_url(url, "pull requires a valid HTTP or HTTPS URL")?;
    let dest = nas_path(dest)?;
    let deadline = Deadline::new(timeout, "NAS download task did not complete")?;
    ensure_dir(client, dest, &deadline)?;
    let form = [
        ("is_batch", "false"),
        ("save_dir", dest),
        ("download_url", url),
        ("force", if force { "true" } else { "false" }),
    ];
    let request_timeout = deadline.remaining()?.min(FILE_TIMEOUT);
    let response = client.request(
        Method::POST,
        "/ugreen/v1/downloadCenter/download/add",
        |request| request.timeout(request_timeout).form(&form),
    )?;
    let task = data(&response);
    let mut result = json!({"save_dir": dest, "task": task});
    if !wait {
        return Ok(result);
    }
    let task_id = first_text(task, &["task_id", "id", "taskId"]);
    let task_name = first_text(task, &["task_name", "taskName", "name"]);
    if task_id.is_none() && task_name.is_none() {
        return Err(Error::Other(
            "NAS queued the download without returning a task ID or name".into(),
        ));
    }
    loop {
        let request_timeout = deadline.remaining()?.min(FILE_TIMEOUT);
        let completed = download_list(client, true, request_timeout)?;
        if let Some(item) = items(&completed, 0)
            .unwrap_or(EMPTY)
            .iter()
            .find(|item| matches_task(item, task_id.as_deref(), task_name.as_deref()))
        {
            let folder =
                first_text(item, &["save_dir", "savePath"]).unwrap_or_else(|| dest.to_owned());
            let filename = first_text(
                item,
                &["download_file_name", "fileName", "task_name", "taskName"],
            );
            let path = match filename {
                Some(name) if basename(&folder) != name => {
                    format!("{}/{}", folder.trim_end_matches('/'), name)
                }
                _ => folder,
            };
            result["nas_path"] = json!(path);
            return Ok(result);
        }
        deadline.pause(Duration::from_secs(3))?;
    }
}

pub(crate) fn downloads(client: &mut NasClient, completed: bool) -> Result<Value> {
    let payload = download_list(client, completed, FILE_TIMEOUT)?;
    Ok(match items(&payload, 0) {
        Some(tasks) => json!({"items": tasks, "completed": completed}),
        None => json!({"items": [], "completed": completed, "data": data(&payload)}),
    })
}

fn download_list(client: &mut NasClient, completed: bool, timeout: Duration) -> Result<Value> {
    let path = if completed {
        "/ugreen/v1/downloadCenter/complete/getList"
    } else {
        "/ugreen/v1/downloadCenter/download/getListV3"
    };
    client.request(Method::GET, path, |request| request.timeout(timeout))
}

fn matches_task(item: &Value, task_id: Option<&str>, task_name: Option<&str>) -> bool {
    let item_id = first_text(item, &["task_id", "id", "taskId"]);
    if let (Some(expected), Some(actual)) = (task_id, item_id.as_deref()) {
        return expected == actual;
    }
    let item_name = first_text(
        item,
        &[
            "task_name",
            "taskName",
            "download_file_name",
            "fileName",
            "name",
        ],
    );
    task_name.is_some() && task_name == item_name.as_deref()
}

pub(crate) fn share(
    client: &mut NasClient,
    paths: &[String],
    password: Option<&str>,
    days: u32,
    public_base: Option<&str>,
) -> Result<Value> {
    validate_paths(paths)?;
    let public_base = public_base.map(checked_public_base).transpose()?;
    let mut body = json!({
        "share_type": 2, "paths": paths, "allow_download": true,
        "internal_user_names": [], "internal_group_names": [], "expiration_time": days,
    });
    if let Some(password) = password.filter(|value| !value.is_empty()) {
        body["password"] = json!(password);
    }
    let payload = client.request(Method::POST, "/ugreen/v1/filemgr/addShare", |request| {
        request.json(&body)
    })?;
    let mut created = data(&payload).as_object().cloned().ok_or_else(|| {
        Error::Other("NAS returned unexpected share metadata after creating the share".into())
    })?;
    let mut link = first_text(data(&payload), &["link", "share_link"]);
    let id = first_text(data(&payload), &["id", "share_id"]);
    if link.is_none()
        && let Some(id) = id.as_deref()
        && let Ok(details) =
            client.request(Method::GET, "/ugreen/v1/filemgr/getShareMsg", |request| {
                request.query(&[("id", id)])
            })
    {
        let details = data(&details);
        link = first_text(details, &["link", "share_link"]);
        if let Some(fields) = details.as_object() {
            created.extend(fields.clone());
        }
    }
    let direct_id = created.get("share_id").and_then(text_value).or_else(|| {
        link.as_deref()
            .and_then(|link| share_reference(link).ok().map(|(id, _)| id))
    });
    if let Some(ref public_id) = direct_id {
        created.insert("share_id".into(), json!(public_id));
        if link.is_none()
            && let Some(base) = public_base
        {
            link = Some(make_share_page(&base, public_id)?.into());
        }
    }
    if let Some(link) = link {
        created.insert("link".into(), json!(link));
    }
    Ok(Value::Object(created))
}

pub(crate) fn list_shares(
    client: &mut NasClient,
    keyword: Option<&str>,
    page: u32,
    limit: u32,
) -> Result<Value> {
    validate_page(page, limit)?;
    let payload = share_list(client, keyword, page, limit)?;
    Ok(json!({"page": page, "limit": limit, "items": items(&payload, 0).unwrap_or(EMPTY)}))
}

fn share_list(
    client: &mut NasClient,
    keyword: Option<&str>,
    page: u32,
    limit: u32,
) -> Result<Value> {
    client.request(Method::GET, "/ugreen/v1/filemgr/shareList", |request| {
        let mut request = request.query(&[("page", page), ("limit", limit)]);
        if let Some(keyword) = keyword.filter(|value| !value.is_empty()) {
            request = request.query(&[("key_word", keyword)]);
        }
        request
    })
}

pub(crate) fn unshare(client: &mut NasClient, refs: &[String]) -> Result<Value> {
    if refs.is_empty() {
        return Err(Error::Usage(
            "at least one share ID or URL is required".into(),
        ));
    }
    let mut resolved = Vec::with_capacity(refs.len());
    let mut public_ids = Vec::new();
    for reference in refs {
        let reference = reference.trim();
        if !reference.is_empty() && reference.bytes().all(|byte| byte.is_ascii_digit()) {
            if !resolved.iter().any(|id| id == reference) {
                resolved.push(reference.to_owned());
            }
        } else {
            let (id, _) = share_reference(reference)?;
            public_ids.push(id);
        }
    }
    if !public_ids.is_empty() {
        let shares = collect_pages(1, 100, true, |page| share_list(client, None, page, 100))?;
        for public_id in public_ids {
            let mut found = None;
            for item in &shares {
                let stored_id = first_text(item, &["share_id"]);
                let link = first_text(item, &["share_link", "link"]);
                let link_id = link.and_then(|value| share_reference(&value).ok().map(|(id, _)| id));
                if stored_id.as_deref() != Some(public_id.as_str())
                    && link_id.as_deref() != Some(public_id.as_str())
                {
                    continue;
                }
                let Some(id) = first_text(item, &["id"])
                    .filter(|id| id.bytes().all(|byte| byte.is_ascii_digit()))
                else {
                    continue;
                };
                if found.as_ref().is_some_and(|previous| previous != &id) {
                    return Err(Error::Other(
                        "share reference matched multiple active shares".into(),
                    ));
                }
                found = Some(id);
            }
            let id = found.ok_or_else(|| {
                Error::Other("share ID was not found in the NAS share list".into())
            })?;
            if !resolved.contains(&id) {
                resolved.push(id);
            }
        }
    }
    let ids = resolved.join(",");
    client.request(Method::DELETE, "/ugreen/v1/filemgr/delShare", |request| {
        request.query(&[("id", &ids)])
    })
}

pub(crate) fn share_reference(reference: &str) -> Result<(String, Option<String>)> {
    let reference = reference.trim();
    if !reference.contains(['/', '?', '#', ':']) {
        return valid_share_id(reference).map(|id| (id, None));
    }
    let url = validate_http_url(reference, "share page must use an HTTP or HTTPS URL")?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::Usage(
            "share page URL must omit user credentials".into(),
        ));
    }
    let id = ["id", "share_id"]
        .into_iter()
        .find_map(|key| {
            url.query_pairs()
                .find(|(name, value)| name == key && !value.is_empty())
                .map(|(_, value)| value.into_owned())
        })
        .ok_or_else(|| {
            Error::Usage("share page URL must contain an id or share_id parameter".into())
        })?;
    let host = url.host_str().unwrap_or_default();
    let origin = if is_uglink_subdomain(host) {
        Some(format!("https://{host}"))
    } else if host == "ug.link" {
        url.path_segments()
            .and_then(|mut paths| paths.next())
            .filter(|account| {
                !account.is_empty()
                    && *account != "filemgr"
                    && account
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
            .map(|account| format!("https://{account}.ug.link"))
    } else {
        None
    };
    Ok((valid_share_id(&id)?, origin))
}

pub(crate) fn share_direct(
    client: &mut NasClient,
    reference: &str,
    password: &str,
    paths: &[String],
    public_base: Option<&str>,
) -> Result<Value> {
    if !paths.is_empty() {
        validate_paths(paths)?;
    }
    let (id, detected_base) = share_reference(reference)?;
    if detected_base.is_none() && public_base.is_none() && reference.contains("://") {
        return Err(Error::Usage(
            "set --public-base-url to use a custom share page".into(),
        ));
    }
    let base = public_base
        .or(detected_base.as_deref())
        .map(checked_public_base)
        .transpose()?
        .map_or_else(|| public_base_from_nas(client), Ok)?;
    let origin = base.origin().ascii_serialization();
    let referer = make_share_page(&base, &id)?;
    let page = |method, path| -> Result<_> {
        Ok(client
            .http()
            .request(method, share_endpoint(&base, path)?)
            .timeout(FILE_TIMEOUT)
            .header("UG-Agent", "PC/WEB")
            .header("X-Specify-Language", "zh-CN")
            .header(ORIGIN, &origin)
            .header(REFERER, referer.as_str()))
    };
    let info = check_response(
        page(Method::GET, "queryShareInfo")?
            .query(&[("share_id", &id)])
            .send()?,
    )?;
    let info = data(&info);
    let verified = check_response(
        page(Method::POST, "externalVerifySharePassword")?
            .json(&json!({"share_id": id, "password": password, "no_count": false}))
            .send()?,
    )?;
    let files = data(&verified)["file_info"]
        .as_array()
        .map_or(EMPTY, Vec::as_slice);
    let inferred_paths = if paths.is_empty() {
        let mut inferred: Vec<String> = files
            .iter()
            .filter_map(|item| first_text(item, &["path"]))
            .collect();
        if inferred.is_empty()
            && let Some(path) = first_text(info, &["path"])
        {
            inferred.push(path);
        }
        if inferred.is_empty() {
            return Err(Error::Other(
                "share response did not include downloadable paths".into(),
            ));
        }
        inferred
    } else {
        Vec::new()
    };
    let paths = if paths.is_empty() {
        &inferred_paths
    } else {
        paths
    };
    let task = check_response(
        page(Method::POST, "addPathsByShareId")?
            .json(&json!({"share_id": id, "paths": paths}))
            .send()?,
    )?;
    let task_id = first_text(data(&task), &["result", "task_id"])
        .ok_or_else(|| Error::Other("share response did not include a download task ID".into()))?;
    let selected_file = if let [path] = paths {
        files
            .iter()
            .find(|item| item["path"].as_str() == Some(path))
    } else {
        None
    };
    let name = if let [path] = paths {
        selected_file
            .and_then(|file| first_text(file, &["name"]))
            .or_else(|| Some(basename(path).to_owned()).filter(|name| !name.is_empty()))
            .or_else(|| first_text(info, &["name"]))
    } else {
        None
    };
    let mut link = share_endpoint(&base, "shareDownloadFile")?;
    link.query_pairs_mut().extend_pairs([
        ("coding", "true"),
        ("share_id", id.as_str()),
        ("password", password),
        ("task_id", task_id.as_str()),
    ]);
    Ok(json!({
        "url": link.as_str(), "task_id": task_id, "share_id": id, "paths": paths,
        "name": name, "size": selected_file.map(|file| &file["size"]), "files": files,
        "share_info": info,
    }))
}

fn public_base_from_nas(client: &NasClient) -> Result<Url> {
    let from_login = client.login_data()["uglink"]
        .as_str()
        .filter(|value| !value.is_empty())
        .and_then(|link| {
            let link = if link.contains("://") {
                link.to_owned()
            } else {
                format!("https://{link}")
            };
            Url::parse(&link).ok()
        });
    for base in from_login.into_iter().chain(client.url("/").ok()) {
        if let Some(host) = base.host_str().filter(|host| is_uglink_subdomain(host)) {
            return checked_public_base(&format!("https://{host}"));
        }
    }
    Err(Error::Usage(
        "set --public-base-url or provide a UG Link share page".into(),
    ))
}

fn checked_public_base(base: &str) -> Result<Url> {
    let url = validate_http_url(base, "public base must be an HTTP or HTTPS origin")?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path().trim_end_matches('/'), "" | "/ugreen/v1")
    {
        return Err(Error::Usage(
            "public base must be an HTTP or HTTPS origin without credentials or query parameters"
                .into(),
        ));
    }
    Ok(url)
}

fn share_endpoint(base: &Url, endpoint: &str) -> Result<Url> {
    base.join(&format!("/ugreen/v1/filemgr/{endpoint}"))
        .map_err(|_| Error::Other("cannot build the public share endpoint".into()))
}

fn make_share_page(base: &Url, id: &str) -> Result<Url> {
    let mut page = base
        .join("/filemgr/share-download/")
        .map_err(|_| Error::Other("cannot build the share page URL".into()))?;
    page.query_pairs_mut().append_pair("id", id);
    Ok(page)
}

fn valid_share_id(id: &str) -> Result<String> {
    if id.is_empty()
        || id
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(Error::Usage(
            "share ID is empty or contains invalid characters".into(),
        ));
    }
    Ok(id.to_owned())
}

fn is_uglink_subdomain(host: &str) -> bool {
    host.strip_suffix(".ug.link")
        .is_some_and(|account| !account.is_empty())
}

fn validate_http_url(url: &str, message: &str) -> Result<Url> {
    let url = Url::parse(url).map_err(|_| Error::Usage(message.into()))?;
    if !matches!(url.scheme(), "http" | "https") || !url.has_host() {
        return Err(Error::Usage(message.into()));
    }
    Ok(url)
}

fn nas_path(path: &str) -> Result<&str> {
    if !path.starts_with('/')
        || path.contains('\0')
        || path.split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err(Error::Usage(
            "NAS paths must be absolute and omit dot segments".into(),
        ));
    }
    let normalized = path.trim_end_matches('/');
    Ok(if normalized.is_empty() {
        "/"
    } else {
        normalized
    })
}

fn non_root_path(path: &str) -> Result<&str> {
    let path = nas_path(path)?;
    if path == "/" {
        return Err(Error::Usage(
            "this operation requires a path below the NAS root".into(),
        ));
    }
    Ok(path)
}

fn validate_paths(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Err(Error::Usage("at least one NAS path is required".into()));
    }
    for path in paths {
        non_root_path(path)?;
    }
    Ok(())
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

fn first_text(data: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| data.get(key).and_then(text_value))
}

fn text_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.is_empty() => Some(value.to_owned()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn unsigned(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

fn validate_page(page: u32, limit: u32) -> Result<()> {
    if page == 0 || limit == 0 {
        return Err(Error::Usage(
            "page and limit must be greater than zero".into(),
        ));
    }
    Ok(())
}

fn items(value: &Value, depth: u8) -> Option<&[Value]> {
    if depth > 4 {
        return None;
    }
    if let Some(items) = value.as_array() {
        return Some(items);
    }
    [
        "result",
        "list",
        "records",
        "items",
        "rows",
        "tasks",
        "data",
        "files",
        "right_files",
        "tree",
    ]
    .into_iter()
    .filter_map(|key| value.get(key))
    .find_map(|nested| items(nested, depth + 1))
}

fn total_count(value: &Value, depth: u8) -> Option<u64> {
    if depth > 4 {
        return None;
    }
    unsigned(&value["total"]).or_else(|| {
        ["data", "result", "files", "right_files"]
            .into_iter()
            .filter_map(|key| value.get(key))
            .find_map(|nested| total_count(nested, depth + 1))
    })
}

fn collect_pages(
    mut page: u32,
    limit: u32,
    all: bool,
    mut request: impl FnMut(u32) -> Result<Value>,
) -> Result<Vec<Value>> {
    validate_page(page, limit)?;
    let start_offset = u64::from(page - 1) * u64::from(limit);
    let mut result = Vec::new();
    let mut previous_size = 0;
    loop {
        let payload = request(page)?;
        let batch = items(&payload, 0).unwrap_or(EMPTY);
        if all && previous_size != 0 && batch.len() == previous_size && result.ends_with(batch) {
            return Err(Error::Other(
                "NAS returned the same results for successive pages".into(),
            ));
        }
        previous_size = batch.len();
        result.extend_from_slice(batch);
        let parsed_through = start_offset.saturating_add(result.len() as u64);
        let has_more = total_count(&payload, 0).map_or(batch.len() >= limit as usize, |total| {
            total > parsed_through
        });
        if !all || batch.is_empty() || !has_more {
            return Ok(result);
        }
        page = page
            .checked_add(1)
            .ok_or_else(|| Error::Other("NAS pagination exceeded the page range".into()))?;
    }
}

struct Deadline {
    start: Instant,
    timeout: Duration,
    message: &'static str,
}

impl Deadline {
    fn new(timeout: Duration, message: &'static str) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Usage("timeout must be greater than zero".into()));
        }
        Ok(Self {
            start: Instant::now(),
            timeout,
            message,
        })
    }

    fn remaining(&self) -> Result<Duration> {
        let remaining = self.timeout.saturating_sub(self.start.elapsed());
        if remaining.is_zero() {
            return Err(Error::Timeout(format!(
                "{} within {}s",
                self.message,
                self.timeout.as_secs_f64()
            )));
        }
        Ok(remaining)
    }

    fn pause(&self, interval: Duration) -> Result<()> {
        thread::sleep(self.remaining()?.min(interval));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_vendor_share_urls_by_host() {
        assert_eq!(share_reference("abc").unwrap(), ("abc".into(), None));
        assert_eq!(
            share_reference("http://ug.link/my-account/filemgr/share-download/?id=abc").unwrap(),
            ("abc".into(), Some("https://my-account.ug.link".into()))
        );
        assert_eq!(
            share_reference("https://x.ug.link/share?share_id=a%2Bb").unwrap(),
            ("a+b".into(), Some("https://x.ug.link".into()))
        );
        assert_eq!(
            share_reference("https://x.ug.link.evil.test/share?id=abc").unwrap(),
            ("abc".into(), None)
        );
        assert_eq!(
            share_reference("http://nas.local:9443/share?id=abc").unwrap(),
            ("abc".into(), None)
        );
        assert!(share_reference("https://x.ug.link@evil.test/share?id=abc").is_err());
        assert!(share_reference("https://ug.link/share").is_err());
    }

    #[test]
    fn collects_wrapped_pages_using_reported_total() {
        let mut pages = Vec::new();
        let output = collect_pages(1, 2, true, |page| {
            pages.push(page);
            Ok(match page {
                1 => json!({"data": {"right_files": {"files": {"data": [{"name": "a"}]}, "total": 3}}}),
                2 => json!({"data": {"result": [{"name": "b"}], "total": 3}}),
                3 => json!({"data": {"result": [{"name": "c"}], "total": 3}}),
                _ => panic!("unexpected page {page}"),
            })
        }).unwrap();
        assert_eq!(pages, [1, 2, 3]);
        assert_eq!(
            output,
            [
                json!({"name": "a"}),
                json!({"name": "b"}),
                json!({"name": "c"})
            ]
        );
    }

    #[test]
    fn detects_repeated_pages() {
        let result = collect_pages(1, 1, true, |_| Ok(json!({"result": [{"name": "a"}]})));
        assert!(matches!(result, Err(Error::Other(_))));
    }

    #[test]
    fn completed_task_matching_prefers_identifier_and_uses_exact_names() {
        let item = json!({"id": 11, "task_name": "a.zip.backup"});
        assert!(matches_task(&item, Some("11"), Some("a.zip")));
        assert!(!matches_task(&item, Some("12"), Some("a.zip.backup")));
        assert!(!matches_task(&item, None, Some("a.zip")));
        assert!(matches_task(&item, None, Some("a.zip.backup")));
    }
}
