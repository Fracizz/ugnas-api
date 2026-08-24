# 登录 / 下载 / 分享 / 获取分享地址

按官方 PC 客户端 1.18.0 文件管理器前端还原。  
局域网 API 入口是 `http://<nas-ip>:9999/ugreen/`（80/443 回的是前端页面，不是 JSON）。  
不要把 token、密码、分享码贴到公开位置。

和「共享文件夹 / SMB」不是同一套：

| 你要的 | 接口 | 不是这个 |
|---|---|---|
| 文件外链分享、复制链接 | `addShare` / `getShareMsg` / `shareList` | `createShareFolder`、`v1/filemgr/shares` |
| 邀请用户进 NAS | — | 控制面板 `getShareLink`（uglink/DDNS 邀请） |
| 文件收集箱 | — | `createFileCollection`（也有 `link`，但是收集上传） |

## 1. 登录

```text
POST /ugreen/v1/verify/check     { "username" }
        └─ 响应头 x-rsa-token（Base64 公钥）
RSA/PKCS#1 v1.5 加密密码
POST /ugreen/v1/verify/login
PUT  /ugreen/v1/verify/refresh   续期
GET  /ugreen/v1/verify/is_login  探会话
```

登录请求头加 `UG-Client-Id`。Body：

```json
{
  "username": "<redacted>",
  "password": "<rsa-ciphertext>",
  "keepalive": true,
  "otp": false,
  "is_simple": false
}
```

登录 `data` 里常用字段：

- `token`：后续鉴权
- `auth_type`：这台设备返回 `"header"` → 请求头 `X-Ugreen-Token`；若为 `"url"` 则用查询参数 `?token=`
- `public_key`：后续可选的报文加密公钥
- `static_token`

公共头：`UG-Agent: PC/WEB`、`Client-Id`、`Client-Version`、`X-Specify-Language`。

登录成功后客户端会把 `token` / `auth_type` / `uglink` / 端口写到
`~/.ugnas/profiles/<profile>/session.json`（Windows 上 token 由 DPAPI 加密）。下次 `login()` 先复用会话；code `1024` 先 refresh，失败再走 RSA 登录。

## 2. 下载

`save_dir` 是下载中心表单字段，一开始就有。Python 里用 `add_http_download(url, save_dir)` 显式传入，不要写死在客户端里。本地下载用 `download_file(nas_path, dest)` 或 `download_url_to_file(url, dest)`。

### 下载中心：让 NAS 自己拉 HTTP 链接

对应桌面端「下载中心 → 新建下载任务」。

`POST /ugreen/v1/downloadCenter/download/add`（form）

```
is_batch=false
save_dir=/volume1/技术平台-研发效能/临时/下载
download_url=http://...
force=true
```

成功时 `data.status=0`，`task_status=1` 表示排队/下载中。  
进度：`GET /ugreen/v1/downloadCenter/download/getListV3`  
完成：`GET /ugreen/v1/downloadCenter/complete/getList`

同名文件先 `POST /ugreen/v1/filemgr/delPaths`。

### 已登录后下载 NAS 上的文件

官方走两步，第二步可以不带登录 token。

### 取下载令牌

`GET /ugreen/v2/filemgr/getDownloadToken?paths=<urlencode>&paths=<urlencode>`

Query：

| 参数 | 说明 |
|---|---|
| `paths` | 可重复，每个文件/目录一条 |
| `intranet_share_id` | 内网分享场景，普通下载给 `0` |
| `coding` | 客户端传 `true` |
| `is_livephoto` | Live Photo 才用 |

返回：`data.dl_token`。

### 拉文件

`GET /ugreen/v2/filemgr/downloadFile?token=<dl_token>`

- `tokenAuth: false`，不要再叠登录 token
- 二进制流，不是 JSON

v2 失败时客户端回退：

```text
GET /ugreen/v1/filemgr/downloadFile?paths=<path>&token=<api_token>&coding=true
GET /ugreen/v1/filemgr/downloadFile?intranet_share_id=0&paths=<path>
```

桌面端本地下载还会走 `transportCore.downloadFile`，URL 仍是上面这一支。

分享页直链是 `GET /ugreen/v1/filemgr/shareDownloadFile?coding=true&share_id=&password=&task_id=`。用 `get_share_download_url(share_id)` 签发一次性 `task_id`；已有 task 时用 `share_download_file_url` 拼 URL，再 `download_url_to_file(url, dest)` 落到本机。`--dry-run` 只 HEAD / Range，不拉整包。

## 3. 分享（创建外链）

先看权限：`GET /ugreen/v1/filemgr/sharePermission` 或 `GET /ugreen/v2/filemgr/shareUpFilePermission`。

`POST /ugreen/v1/filemgr/addShare`

```json
{
  "share_type": 2,
  "paths": ["/volume1/example/photo.jpg"],
  "allow_download": true,
  "internal_user_names": [],
  "internal_group_names": [],
  "expiration_time": 0,
  "password": "ab12",
  "visit_limit": 1000
}
```

| 字段 | 含义 |
|---|---|
| `share_type` | `1` 指定内部用户/组；`2` 公开链接（要分享地址用这个） |
| `paths` | NAS 绝对路径数组 |
| `allow_download` | 是否允许下载 |
| `expiration_time` | 公开链才有。`0` 永久；否则是「距今天的天数」，不是 Unix 时间 |
| `password` | 可选，4–8 位字母数字。回读时是 Base64 |
| `visit_limit` | 可选访问次数。未启用时客户端用 `-1` |
| `internal_user_names` / `internal_group_names` | `share_type=1` 时用 |

成功：`data.link` 就是分享地址。同时带 `id`、`ext`、`allow_download` 等。

修改：`POST /ugreen/v1/filemgr/editShare`，body 带 `id`，只传改过的字段。  
删除：`DELETE /ugreen/v1/filemgr/delShare?id=1`（多个 id 逗号拼接）。  
`shareList` 里的 `id` 是数字；分享页链接 `id=` 是 hex。`unshare()` 两种都能解析。

## 3.1 文件增删改（库已封装）

```text
POST /ugreen/v2/filemgr/createFolder     {"path":"/volume1/share/tmp","no_recursive_creation":false}
POST /ugreen/v2/filemgr/rename           {"path":"/volume1/share/a.zip","new_name":"b.zip"}
POST /ugreen/v2/filemgr/cpOrMvPath       {"dst":"/volume1/share/tmp","src":["/volume1/share/b.zip"],"action_type":1,"type":true,"intranet_share_id":0}
POST /ugreen/v1/filemgr/delPaths         {"paths":["/volume1/share/tmp/b.zip"]}
POST /ugreen/v1/filemgr/fileUpload       multipart：空文件预检，first_request=true
POST /ugreen/v1/filemgr/fileUploadV2     application/octet-stream，上传参数放在 ug-param 请求头
GET  /ugreen/v1/filemgr/getUpdateTmpInfo 查询 NAS 已确认的上传偏移，用于断点续传
```

`cpOrMvPath` 的 `type=true` 是移动，`false` 是复制；`action_type`：`0` 跳过、`1` 覆盖。复制/移动可能先返回 `task_id`，要等目标路径出现。  
`createFolder` 的 `path` 是要创建的完整路径，不是「父目录 + name」。

## 3.2 剩余空间（库已封装）

```text
GET  /ugreen/v1/filemgr/getVolumes          data.result[].free / used / all
POST /ugreen/v1/filemgr/getPathFreeSize     {"path":"/volume1/share"} → data.result
GET  /ugreen/v2/user/directory/quota        用户目录限额（未启用时 is_enable=false）
```

桌面端 `getStorages()` 把 `free`/`all` 映射成 `available`/`total`。卷根路径（如 `/volume1`）有时会 1301，`nas.free()` 会回退到对应卷的 `free`。

目录占用（属性窗口 `startQuerySize`）：

```text
GET /ugreen/v2/filemgr/getSizeAndCount?path=<path>&intranet_share_id=0
```

第一次 `status=0`、`size=0`，带 `calculate_id`；轮询到 `status=1` 后有 `size` / `takes_space` / `file_count` / `dir_count`。网页端会给 query 做 `encrypt_query`，带登录 `token` 的明文 `path` 同样可用。`getFileInfo` 对文件夹的 `size` 恒为 0，不要用它当占用。

登录态：本地 session 复用后用 `getVolumes` 探活（不要用 heartbeat）。接口 1024 / HTTP 401 时先 `refresh`，再密码重登并重放请求。

## 4. 获取分享地址

已有分享，不要再 `addShare`，查详情或列表：

```text
GET /ugreen/v1/filemgr/getShareMsg?id=<share_id>
GET /ugreen/v1/filemgr/shareList
GET /ugreen/v1/filemgr/shareList?key_word=<keyword>
```

地址字段都是 `data.link`（列表项里同样叫 `link`）。  
`expire_time` 回读是 Unix 秒；创建时的 `expiration_time` 是天数，不要混。

### 外链直链（`shareDownloadFile`）

分享页 `http://ug.link/<uglink_id>/filemgr/share-download/?id=<share_id>` 点下载时，走 uglink 主机，**不需要登录 token / cookie**。Body 必须是紧凑 UTF-8 JSON（`ensure_ascii=False`），不要把中文路径打成 `\uXXXX`。

```text
GET  https://<uglink>/ugreen/v1/filemgr/queryShareInfo?share_id=<share_id>
POST https://<uglink>/ugreen/v1/filemgr/externalVerifySharePassword
     {"share_id":"<id>","password":"","no_count":false}
POST https://<uglink>/ugreen/v1/filemgr/addPathsByShareId
     {"paths":["/volume1/.../file.zip"],"share_id":"<id>"}
        └─ data.result = task_id
GET  https://<uglink>/ugreen/v1/filemgr/shareDownloadFile?coding=true&share_id=<id>&password=&task_id=<task_id>
```

`task_id` 每次点下载都会新发，绑定本次要下的 paths，不要当成永久链接缓存。  
`nas.share_direct()` 可直接吃分享页 URL（解析 `id=`），不必先拆 share_id。  
`nas.download_share(link, dest)`：现场签发直链并立刻落到本机。

三种地址：

| 地址 | 过期 | 库内处理 |
| --- | --- | --- |
| 分享页 `data.link` | `expiration_time=0` 长期；否则按天数。取消分享或 `visit_limit` 用尽也会失效 | `share` / `share_file` 返回 `link`、`share_id`，给人保存 |
| `shareDownloadFile?...&task_id=` | 一次性 | `share_direct` 每次新签；`download_share` 签完就下 |
| `downloadFile?token=`（`dl_token`） | 短时 | `download()` 每次 `getDownloadToken` 再拉；登录 1024/401 会 refresh 或重登 |

公共头：`UG-Agent: PC/WEB`、`X-Specify-Language`、`Origin` / `Referer` 指向分享页。

## Python

安装与基本用法见 [usage.md](usage.md) 和仓库根 [README.md](../README.md)。对外入口：`from ugnas import Ugnas`。  
底层仍是 `UgnasClient`。不要把 config / session / `.env` 提交进仓库。
