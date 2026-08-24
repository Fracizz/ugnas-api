# UGOS Pro login and file-list API

This is an English summary of the protocol observations used by `ugnas-api`.
The interface is undocumented and may vary by NAS firmware. No credentials,
cookies, tokens, device IDs, or captures belong in this document.

## Base URL and common headers

The LAN JSON entry point is usually:

```text
http://<nas-ip>:9999/ugreen/
```

The web UI commonly uses ports 80 or 443 instead. Successful responses
generally look like:

```json
{ "code": 200, "msg": "...", "data": {} }
```

Typical request headers are `UG-Agent: PC/WEB`, `Client-Id`,
`Client-Version`, and `X-Specify-Language: en-US`. Authentication is either
`X-Ugreen-Token` or a `token` query parameter, depending on the device's
`auth_type` response field.

## Login flow

```text
POST /ugreen/v1/verify/check     { "username": "<redacted>" }
        └─ response header x-rsa-token (Base64 public key)
RSA/PKCS#1 v1.5 encrypt the password
POST /ugreen/v1/verify/login
PUT  /ugreen/v1/verify/refresh
GET  /ugreen/v1/verify/is_login
```

The login body includes `username`, encrypted `password`, `keepalive`, `otp`,
and `is_simple`. Login data supplies the API token, `auth_type`, optional
static token, public key, and NAS information. The client stores only a
protected session representation and refreshes or re-authenticates when code
1024 or HTTP 401 indicates expiry.

## File listing

Recommended sequence:

```text
GET  /ugreen/v2/filemgr/getRootDirs
POST /ugreen/v2/filemgr/getDirFileListV2
```

The v2 listing body normally contains `path`, `page`, `limit`, `data_type`,
`show_dir`, `left_no_page_show`, `left_count`, and `is_shield_recycle`.
Older firmware may still expose:

```text
POST /ugreen/v1/filemgr/getDirFileList
```

Use NAS absolute paths such as `/volume1/share`. Do not assume pagination
semantics or response field names are stable across firmware versions.

## Related endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/ugreen/v2/filemgr/getFileInfo` | File metadata |
| GET | `/ugreen/v2/filemgr/getDownloadToken` | Short-lived download token |
| POST | `/ugreen/v2/filemgr/createFolder` | Create a directory |
| POST | `/ugreen/v2/filemgr/rename` | Rename a path |
| POST | `/ugreen/v2/filemgr/cpOrMvPath` | Copy or move paths |
| POST | `/ugreen/v1/filemgr/delPaths` | Delete paths |
| GET | `/ugreen/v1/filemgr/getShareList` | List shares |

For the detailed Chinese notes, see
[api-login-filelist.md](api-login-filelist.md).
