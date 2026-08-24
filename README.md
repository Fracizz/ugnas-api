# ugnas-api

Unofficial Python client for local UGOS Pro / UGREEN NAS APIs. It provides a
typed facade for authentication, file management, NAS-side downloads,
external sharing, storage queries, and resilient transfers.

> **Disclaimer:** This project is independent and is not affiliated with or
> endorsed by UGREEN. The API is undocumented and may change between firmware
> releases.

[中文说明](README.zh-CN.md) · [English usage guide](docs/usage.en.md) · [Changelog](CHANGELOG.md) · [Security](SECURITY.md)

## Highlights

- RSA-protected login with reusable, profile-isolated sessions.
- File listing, creation, moves, copies, renames, deletion, uploads, and downloads.
- Resumable uploads and downloads with progress and cancellation hooks.
- NAS-side HTTP downloads for large files.
- External share creation, one-shot direct URLs, and safe shared-file downloads.
- Volume capacity, path free-space, and asynchronous directory-size queries.
- Python API and the `ugnas` command-line interface.

## Installation

```powershell
python -m pip install "ugnas-api @ git+https://github.com/Fracizz/ugnas-api.git"
```

For local development:

```powershell
python -m pip install -e .
```

## Quick start

The usual LAN JSON endpoint is `http://<NAS-IP>:9999`; ports 80 and 443
normally serve the web UI rather than the JSON API.

```python
from ugnas import Ugnas

nas = Ugnas.connect(
    base_url="http://192.168.x.x:9999",
    username="local_user",
    password="read-from-a-secret-store",
    save_dir="/volume1/share/downloads",
    insecure=True,  # only for HTTP or a trusted self-signed endpoint
)

print(nas.list("/volume1/share"))
nas.download("/volume1/share/archive.zip", r"C:\tmp\archive.zip")
```

Do not put real passwords in source code. See the English configuration and
development guide in [docs/usage.md](docs/usage.md).

## 中文详细说明：账号怎么传

密码不要写进 JSON。优先级从高到低：**`connect()` 参数 → 环境变量 / `.env` → 配置文件**。

### 1. 代码里直接传（适合当依赖用）

```python
nas = Ugnas.connect(
    base_url="http://192.168.x.x:9999",
    username="local_user",
    password="...",
    save_dir="/volume1/share/下载",
    public_base_url="https://xxx.ug.link",  # 外链分享需要
    insecure=True,  # 自签证书或纯 HTTP
)
```

`password` 只用于本次登录，不会落盘。登录成功后 token 写到会话文件，下次 `connect()` 会复用；token 过期（1024 / 401）会先续期，失败再用密码自动重登。

### 2. 环境变量 / `.env`

当前目录 `.env`（已 gitignore）：

```env
UGNAS_PASSWORD=
UGNAS_BASE_URL=http://192.168.x.x:9999
UGNAS_USERNAME=local_user
UGNAS_SAVE_DIR=/volume1/share/下载
UGNAS_PUBLIC_BASE_URL=https://xxx.ug.link
UGNAS_INSECURE=1
```

```python
nas = Ugnas.connect()
```

### 3. 先存配置再连

```powershell
copy .env.example .env
# 编辑 .env，只填 UGNAS_PASSWORD
ugnas config-save --base-url http://192.168.x.x:9999 --username local_user --save-dir "/volume1/share/下载" --public-base-url https://xxx.ug.link --insecure
ugnas login
```

多台 NAS 可分别保存，例如在上述命令追加 `--profile work`，之后连接和执行命令时使用同一个
`--profile work`。

Windows 上 `--password` 使用当前用户 DPAPI 加密保存，不会进 JSON 或明文 `.env`。
命令行优先使用 `--prompt-password` 或 `--password-stdin`，避免密码出现在进程参数中。

| 内容 | 位置（按查找顺序） |
| --- | --- |
| 密码 | Windows DPAPI `credentials.json` / `UGNAS_PASSWORD` / 旧版 `.env` |
| 地址、用户、默认目录 | `connect()` / `UGNAS_*` / `~/.ugnas/profiles/<profile>/config.json` |
| 登录 token | `~/.ugnas/profiles/<profile>/session.json`（Windows 上 DPAPI 加密） |

可用 `--profile work` / `UGNAS_PROFILE=work` 隔离多台 NAS 的配置和会话。旧版
`captures/.ugnas_*` 和 `~/.ugnas/*` 仅在未指定 profile 时兼容读取。

## 中文详细说明：文件操作

NAS 路径用正斜杠，例如 `/volume1/share/a.zip`。

| 方法 | 作用 |
| --- | --- |
| `list(path)` | 列目录 |
| `iter(path)` / `list_all(path)` | 自动翻页遍历目录全部条目 |
| `exists(path)` | 是否存在 |
| `mkdir(path)` | 建目录（缺的父目录一并建） |
| `upload(local, dest_dir)` | 本机 → NAS 目录 |
| `upload_to_path(local, dest_path)` | 本机 → NAS，明确指定完整目标文件路径 |
| `download(nas_path, dest)` | NAS → 本机。`dest` 可以是文件或文件夹（不存在会建） |
| `pull(url, save_dir=None)` | 让 NAS 下载中心去拉 HTTP。目录默认用配置里的 `save_dir` |
| `rename(path, new_name)` | 同目录改名 |
| `move(src, dest_dir)` | 移动到目录 |
| `copy(src, dest_dir)` | 复制到目录 |
| `delete([path, ...])` | 删除 |
| `space()` | 各存储卷总量 / 已用 / 剩余（字节） |
| `free(path=None)` | 某路径剩余空间，默认 `save_dir` |
| `size(path)` | 文件/文件夹占用（属性里的大小，异步统计） |

```python
nas.mkdir("/volume1/share/tmp")
print(nas.exists("/volume1/share/tmp"))
nas.list("/volume1/share")

nas.upload(r"D:\tmp\a.zip", "/volume1/share")              # 得到 .../a.zip
nas.upload_to_path(r"D:\tmp\a.zip", "/volume1/share/b.zip") # 指定 NAS 文件名
nas.rename("/volume1/share/a.zip", "c.zip")
nas.move("/volume1/share/c.zip", "/volume1/share/tmp")
nas.copy("/volume1/share/tmp/c.zip", "/volume1/share/ok")

nas.download("/volume1/share/tmp/c.zip", r"D:\tmp\c.zip")  # 本机文件
nas.download("/volume1/share/tmp/c.zip", r"D:\tmp\dl")     # 本机文件夹，自动建

nas.pull("http://example/file.zip")                        # 用配置里的 save_dir
nas.pull("http://example/file.zip", "/volume1/share/tmp", wait=True)

nas.delete(["/volume1/share/tmp/c.zip"])

print(nas.space())
print(nas.free("/volume1/share"))
print(nas.size("/volume1/share/tmp"))
```

`move` / `copy` / `upload` 默认覆盖同名（`force=False` 则跳过）。

`upload` 使用 UGOS 桌面端同款的两阶段协议：先预检目标文件，再通过 `fileUploadV2`
分块上传。网络中断后会查询 NAS 已确认偏移并继续；下载使用 `.part` 文件，长度校验通过后才原子替换目标。

大目录和需要显示进度的上传可以直接使用高层接口：

```python
for item in nas.iter("/volume1/share", page_size=200):
    print(item)

stop = False
nas.upload_to_path(
    r"D:\tmp\large.zip",
    "/volume1/share/large.zip",
    chunk_size=16 * 1024 * 1024,
    progress=lambda done, total: print(f"{done}/{total}"),
    cancelled=lambda: stop,
)
```

异常均从 `ugnas` 导出。可分别捕获 `ApiError`、`AuthExpired`、`NetworkError`、
`OperationTimeout`、`TransferError` 和 `TransferCancelled`；不需要区分时捕获其基类 `UgnasError`。

## 中文详细说明：分享与下载地址

这是文件外链，不是 SMB 共享文件夹。三种地址过期规则不同，**长期只存分享页或 `share_id`**。

| 地址 | 从哪来 | 会过期吗 | 怎么处理 |
| --- | --- | --- | --- |
| 分享页 `http://ug.link/.../share-download/?id=` | `share` / `share_file` 的 `link` | `expiration_days=0` 长期有效，直到 `unshare` 或次数用尽 | 发给别人、入库；不要当文件直链 |
| 直链 `.../shareDownloadFile?...&task_id=` | `share_direct` / `share_file` 的 `direct.url` | **会。** `task_id` 点一次发一次，用完即失效 | 不要缓存。要下文件时再签一次，或用 `download_share` |
| 登录下载 `.../downloadFile?token=`（`dl_token`） | `nas.download` 内部 | **会。** 短时令牌 | 库里每次现取现拉，不要自己存 URL |

```python
info = nas.share_file("/volume1/share/tmp/c.zip")
print(info["link"])                 # 给人打开的页面，可长期保存
print(info["share_id"])
# info["direct"]["url"] 是一次性的，立刻下可以，不要当永久地址

# 已有分享页 / share_id：现场签发直链
direct = nas.share_direct(info["link"])
direct = nas.share_direct("http://ug.link/xxx/filemgr/share-download/?id=...")

# 推荐：签完立刻落到本机，避免拿着过期直链
nas.download_share(info["link"], r"D:\tmp\c.zip")

# 只建分享、不签发直链
page = nas.share(["/volume1/share/tmp/c.zip"], expiration_days=7, password="ab12")

nas.unshare(info["share_id"])       # 取消；hex 或数字 id 都可以
```

自己账号把 NAS 文件拉到本机用 `nas.download(nas_path, dest)`，每次都会新取 `dl_token`。登录 token 过期（1024）会自动续期或重登。

## 中文详细说明：命令行

```powershell
ugnas login
ugnas pull --url http://example/file.zip --save-dir "/volume1/share/下载" --wait
ugnas upload --path D:\tmp\a.zip --dest /volume1/share --progress
ugnas upload --path D:\tmp\a.zip --dest /volume1/share --name b.zip --no-force
ugnas download --path /volume1/share/a.zip --dest D:\tmp\a.zip
ugnas rename --path /volume1/share/a.zip --name b.zip
ugnas move --path /volume1/share/b.zip --dest /volume1/share/tmp
ugnas copy --path /volume1/share/tmp/b.zip --dest /volume1/share/ok
ugnas share --path /volume1/share/tmp/b.zip
ugnas share-direct --url "http://ug.link/xxx/filemgr/share-download/?id=..."
ugnas download-share --url "http://ug.link/xxx/filemgr/share-download/?id=..." --dest D:\tmp\b.zip
ugnas share-direct --share-id <id> --path /volume1/share/tmp/b.zip
ugnas unshare --share-id <id>
ugnas space
ugnas space --path /volume1/share
ugnas size --path /volume1/share/tmp
```

## 中文详细说明：注意事项

- 局域网请用 `:9999`。HTTPS API 端口以登录返回为准，常见是 `9443`。
- 自签证书或 HTTP 设 `insecure=True` / `UGNAS_INSECURE=1`。
- 大文件更稳的是 `pull`（NAS 自己拉），不要用本机 `upload` 传几 GB。
- 会话会复用；失效会自动续期或重登，无需每次传密码。
- `.env`、config、session 已 gitignore，不要提交。
- CLI 默认脱敏 URL 中的 `password` / `token` / `task_id`；只有明确需要原始一次性直链时才使用 `--show-sensitive`。

更多说明：[docs/usage.en.md](docs/usage.en.md)。中文使用指南：[docs/usage.md](docs/usage.md)。English protocol reference：[docs/api-login-download-share.en.md](docs/api-login-download-share.en.md)。
版本变更：[CHANGELOG.md](CHANGELOG.md)。凭据与脱敏约定：[SECURITY.md](SECURITY.md)。

## License

MIT. See [LICENSE](LICENSE).
