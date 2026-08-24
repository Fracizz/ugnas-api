# UGOS Pro download and sharing API

This English reference summarizes the endpoints implemented by
`ugnas-api`. It describes an undocumented device interface; verify behavior
against the firmware running on your own NAS. Never publish passwords,
cookies, tokens, share IDs, captures, or generated direct URLs.

## Download center

To make the NAS fetch an HTTP URL itself:

```text
POST /ugreen/v1/downloadCenter/download/add
```

The form includes `is_batch=false`, `save_dir`, `download_url`, and
`force=true`. Monitor queued downloads with
`GET /ugreen/v1/downloadCenter/download/getListV3` and completed downloads
with `GET /ugreen/v1/downloadCenter/complete/getList`.

## Download a NAS file

First request a short-lived token:

```text
GET /ugreen/v2/filemgr/getDownloadToken?paths=<urlencoded-path>&intranet_share_id=0
```

Then fetch the binary stream:

```text
GET /ugreen/v2/filemgr/downloadFile?token=<dl_token>
```

The second request does not need the login token. The library falls back to
the v1 download endpoint on devices where the v2 flow is unavailable.

## Create an external share

Check permissions, then create a share:

```text
GET  /ugreen/v1/filemgr/sharePermission
POST /ugreen/v1/filemgr/addShare
```

Example body:

```json
{
  "share_type": 2,
  "paths": ["/volume1/share/example.zip"],
  "allow_download": true,
  "expiration_time": 0,
  "password": "",
  "visit_limit": -1
}
```

`share_type: 2` creates a public file link. `expiration_time: 0` means no
time-based expiry; a positive value is a number of days. The response
contains a share-page `link` and an identifier. Use
`POST /ugreen/v1/filemgr/editShare` to update a share and
`DELETE /ugreen/v1/filemgr/delShare?id=<id>` to remove it.

## Resolve and download a share

Keep the share page URL or share ID and resolve a direct URL only when
needed:

```text
GET  /ugreen/v1/filemgr/getShareMsg?id=<share_id>
GET  /ugreen/v1/filemgr/shareList
POST /ugreen/v1/filemgr/addPathsByShareId
GET  /ugreen/v1/filemgr/shareDownloadFile?coding=true&share_id=<id>&task_id=<task_id>
```

The generated `task_id` is short-lived and tied to the requested paths. It is
not a permanent download URL. `download_share()` signs and downloads in one
operation so stale direct URLs are not cached.

## Storage queries

```text
GET  /ugreen/v1/filemgr/getVolumes
POST /ugreen/v1/filemgr/getPathFreeSize
GET  /ugreen/v2/filemgr/getSizeAndCount?path=<path>&intranet_share_id=0
```

Directory size may be asynchronous: poll using the returned calculation ID
until the operation reports completion. Do not use file metadata size as a
directory-usage measurement.

For the detailed Chinese notes, see
[api-login-download-share.md](api-login-download-share.md).
