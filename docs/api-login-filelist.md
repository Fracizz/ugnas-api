# 绿联云 / UGOS Pro：登录与文件列表 API（脱敏）

登录、下载、分享、获取分享地址见 [api-login-download-share.md](api-login-download-share.md)。

来源：本机 Electron 客户端 `UGREEN NAS` 1.18.0（Electron 28.3.3）缓存前端 `desktop` / `filemgr`，以及进程启动参数。  
文档只描述接口形态，不包含 Cookie、Token、设备 ID、签名或账号。

## 客户端实际抓包情况

官方桌面端 `package.json` 里 `webDebug` 为 `false`，主进程是 bytenode（`main-c.jsc`）。  
因此 `--remote-debugging-port=9222` 会出现在命令行里，但 **9222 不会监听**，Chrome `inspect` 这条路走不通。

更稳妥的两件事：

1. 从 `%APPDATA%\UGREEN_Nas_Pro\.cache` 里的前端包还原 `NasRequest` 调用（本文）。
2. 用 Chromium `--log-net-log` 抓实包核对 Header / Body（研究脚本见 `tools/research/start_netlog_capture.ps1`）。

## 本机入口

桌面端不会直连公网域名，而是把主机名映射到本机随机端口（每次启动会变）：

| 逻辑主机 | 作用 | 本次观察到的映射 |
|---|---|---|
| `pro.ugos.pc` | NAS / UGOS API 隧道 | `127.0.0.1:49878` |
| `proapp.ugnas.cloud` | 桌面壳页面 | `127.0.0.1:49879` |

进程参数里还有：

- `--ignore-certificate-errors`
- `--unsafely-treat-insecure-origin-as-secure=http://pro.ugos.pc,http://proapp.ugnas.cloud`

二开时不要写死这些端口。优先用 NAS 的局域网 JSON 入口，例如 `http://<nas-ip>:9999/ugreen/`（80/443 是网页）。  
`NasRequest` 的路径是相对 `baseURL` 的，完整 URL 形如：

```text
{baseURL}/ugreen/v1/verify/login
```

`baseURL` 在客户端里由 `getActiveBaseUrl()` 决定，本地隧道常见为 `http://pro.ugos.pc`。

## 公共约定

成功响应一般是：

```json
{ "code": 200, "msg": "...", "data": {} }
```

前端把 `200 / 1000 / 1013 / 1118` 也当成部分列表接口的“可接受 code”。  
鉴权失败会回到登录页。

### 请求头

| Header | 何时出现 | 说明 |
|---|---|---|
| `UG-Agent` | 几乎总是 | 桌面端为 `PC/WEB` |
| `UG-Client-Id` | 登录 / 验证码 | 客户端生成的设备标识，不要写死、不要公开 |
| `Client-Id` | 业务请求 | `window.clientId` |
| `Client-Version` | 业务请求 | 桌面端版本号字符串 |
| `X-Specify-Language` | 业务请求 | 如 `zh-CN` |
| `X-Ugreen-Token` | `auth_type != "url"` | 登录后拿到的 `token` |
| `token` 查询参数 | `auth_type == "url"`（默认） | 同一枚 token，放在 URL 上 |
| `X-Ugreen-Auth` | 开启报文加密时 | 里面是 JSON：`publicKey` + `apiToken` |
| `x-rsa-token` | **响应头** | `POST /verify/check` 返回，Base64 公钥 |

`tokenAuth=true` 且本地有加密公钥时，客户端还会用 RSA 把 `X-Ugreen-Token` 加密，并加密请求体（`encrypt` / `decrypt`）。  
自己写调用时先走 **明文 token**；只有官方客户端那种“加密模式”失败时，再补 RSA。

## 登录流程

```text
1. POST /ugreen/v1/verify/check     { username }     -> 响应头 x-rsa-token
2. RSA 加密密码（JSEncrypt / PKCS#1 v1.5，公钥 = atob(x-rsa-token)）
3. POST /ugreen/v1/verify/login     { username, password, keepalive, otp, is_simple }
4. 从 data 取出 token / public_key / 用户与 NAS 信息
5. 之后请求带 X-Ugreen-Token 或 ?token=
6. 会话将过期时 PUT /ugreen/v1/verify/refresh
```

### 1. 取密码公钥

`POST /ugreen/v1/verify/check`

```json
{ "username": "<redacted>" }
```

- 不带 token（`tokenAuth: false`）
- 公钥在响应头 `x-rsa-token`，前端再 `atob`
- 登录成功后的 `data.public_key` 也会再刷一次加密公钥

### 2. 账号密码登录

`POST /ugreen/v1/verify/login`

Header：`UG-Client-Id`

```json
{
  "username": "<redacted>",
  "password": "<rsa-ciphertext>",
  "keepalive": true,
  "otp": "",
  "is_simple": false
}
```

前端把登录 `data` 拆成：

```text
token          -> api_token
auth_type      -> token_where，缺省 "url"
static_token   -> 静态资源 token，缺省等于 token
public_key     -> atob 后作为后续加密公钥
is_ugk
need_bind
system_version
用户资料 / NAS 信息
```

### 3. 其它登录相关

| 方法 | 路径 | 用途 |
|---|---|---|
| `GET` | `/ugreen/v1/verify/is_login` | 检查会话；可带 `params.token` |
| `PUT` | `/ugreen/v1/verify/refresh` | 刷新令牌 |
| `GET` | `/ugreen/v1/verify/heartbeat` | 心跳；本地隧道探活也会打这一支 |
| `POST` | `/ugreen/v1/verify/code/login` | 验证码登录 |
| `POST` | `/ugreen/v1/verify/cloud/login` | 云账号：`ugreen_no` + `access_token` + `keepalive` |
| `GET` | `/ugreen/v1/verify/logout` | 退出 |
| `GET` | `/ugreen/v1/verify/is_running` | 设备是否在线 |
| `GET` | `/ugreen/v1/verify/sn` | 设备 SN |
| `GET` | `/ugreen/v1/verify/identify/country` | 地区 |

云账号登录走另一套 `CloudRequest`，基域包括 `https://api.ugnas.com`、`https://web.ugnas.com/account/login/#/`。  
云 token（`access_token`）和 NAS 本地 `token` 不是同一套，不要混用。

## 文件列表流程

建议顺序：

```text
登录成功
  -> GET  /ugreen/v2/filemgr/getRootDirs          根目录 / 共享
  -> POST /ugreen/v2/filemgr/getDirFileListV2     当前目录（新）
  -> 或 POST /ugreen/v1/filemgr/getDirFileList    当前目录（旧，仍在用）
```

### 根目录

`GET /ugreen/v2/filemgr/getRootDirs`  
缓存里还有 `GET /ugreen/v2/filemgr/getRootFolders`。

### 列目录（v1，仍被桌面端调用）

`POST /ugreen/v1/filemgr/getDirFileList`

```json
{
  "path": "/home/ugreen",
  "page": 1,
  "limit": 5500,
  "sort_type": "name",
  "reverse": false,
  "only_dir": false
}
```

侧栏只取文件夹时：`only_dir: true`。

### 列目录（v2，文件管理器主路径）

`POST /ugreen/v2/filemgr/getDirFileListV2`

```json
{
  "path": "/home/ugreen",
  "page": 1,
  "limit": 100,
  "data_type": 0,
  "show_dir": true,
  "left_no_page_show": false,
  "left_count": 5500,
  "is_shield_recycle": false
}
```

前端把返回值拆成：

```text
tree.files / files.data     文件与文件夹
right_files.permission_mask 当前目录权限
total                       总数
```

具体字段名以实包为准；不要假设分页语义长期不变。

### 相邻文件接口

| 方法 | 路径 | 用途 |
|---|---|---|
| `GET` | `/ugreen/v1/filemgr/getShareList` | 共享列表 |
| `GET` | `/ugreen/v1/filemgr/getHomes` | Home |
| `GET` | `/ugreen/v1/filemgr/getFolderList` | 文件夹 |
| `GET` | `/ugreen/v2/filemgr/getFileInfo` | 单文件信息 |
| `GET` | `/ugreen/v2/filemgr/getDownloadToken` | 下载令牌 |
| `POST` | `/ugreen/v2/filemgr/createFolder` | 建目录，body `{path, no_recursive_creation}` |
| `POST` | `/ugreen/v2/filemgr/rename` | 重命名，`{path, new_name}` |
| `POST` | `/ugreen/v2/filemgr/cpOrMvPath` | 复制 / 移动，`{dst, src, action_type, type}` |
| `POST` | `/ugreen/v1/filemgr/delPaths` | 删除 |
| `POST` | `/ugreen/v1/filemgr/fileUpload` | 空 multipart 预检，创建上传任务 |
| `POST` | `/ugreen/v1/filemgr/fileUploadV2` | 原始字节流上传（库里用这个传文件内容） |
| `GET` | `/ugreen/v2/filemgr/downloadFile` | 下载 |
| `GET` | `/ugreen/v1/filemgr/thumbnail` | 缩略图 |

完整 420 条 `NasRequest` 调用见 `captures/nasrequest_calls.txt`（仅路径，无账号数据）。

## 安全注意

- `token`、`static_token`、`UG-Client-Id`、`x-rsa-token`、云 `access_token` 都是短时凭证。
- 默认 `auth_type=url` 时 token 会出现在查询串里，日志和 HAR 都要当密钥处理。
- 不要把 netlog、HAR、cURL 原文贴到公开位置。
- 二开应对自己的 NAS；不要假设字段或加密开关以后还一样。
