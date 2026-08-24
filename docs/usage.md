# Usage guide

The package is named `ugnas-api`; import it with `from ugnas import Ugnas`.
The [English README](../README.md) contains the quick start, while this guide
keeps the detailed Chinese examples below.

[中文说明](../README.zh-CN.md)

## 安装

```powershell
python -m pip install -e .
# 或
python -m pip install dist\ugnas_api-0.3.0-py3-none-any.whl
```

其它项目 `requirements.txt` / `pyproject.toml`：

```
ugnas-api @ git+https://github.com/Fracizz/ugnas-api.git
```

局域网 API 用 `http://<NAS-IP>:9999`。自签证书或 HTTP 加 `insecure=True`。

## 登录

密码不要写进 JSON。三种方式，后者会被前者覆盖：`connect()` 参数 → 环境变量 / `.env` → 配置文件。

```python
from ugnas import Ugnas

nas = Ugnas.connect(
    base_url="http://192.168.x.x:9999",
    username="local_user",
    password="...",
    save_dir="/volume1/share/下载",
    public_base_url="https://xxx.ug.link",
    insecure=True,
)
```

或先：

```powershell
copy .env.example .env
ugnas config-save --base-url http://192.168.x.x:9999 --username local_user --save-dir "/volume1/share/下载" --insecure
ugnas login
```

多台 NAS 使用不同的 `--profile`，例如 `ugnas config-save --profile work ...` 和
`ugnas login --profile work`。

Windows 上保存的密码使用 DPAPI 加密。配置和 token 默认放在
`~/.ugnas/profiles/<profile>/`；可用 `--profile` 隔离多套 NAS 账号。token 过期会先 `refresh`，失败再用密码重登。

## 文件

NAS 路径用 `/volume1/...`。本机路径用 Windows 路径即可。

```python
nas.list("/volume1/share")
nas.exists("/volume1/share/tmp")
nas.mkdir("/volume1/share/tmp")

nas.upload(r"D:\tmp\a.zip", "/volume1/share")
nas.upload_to_path(r"D:\tmp\a.zip", "/volume1/share/b.zip")
nas.rename("/volume1/share/a.zip", "c.zip")
nas.move("/volume1/share/c.zip", "/volume1/share/tmp")
nas.copy("/volume1/share/tmp/c.zip", "/volume1/share/ok")

nas.download("/volume1/share/tmp/c.zip", r"D:\tmp\c.zip")
nas.download("/volume1/share/tmp/c.zip", r"D:\tmp\dl")   # 文件夹不存在会建

nas.pull("http://example/file.zip")                      # NAS 下载中心
nas.pull("http://example/file.zip", "/volume1/share/tmp", wait=True)

nas.delete(["/volume1/share/tmp/c.zip"])

print(nas.space())                 # 各卷 free / used / total
print(nas.free("/volume1/share"))  # 路径剩余字节
print(nas.size("/volume1/share"))  # 该目录占用 / 文件数
```

| 方法 | 说明 |
| --- | --- |
| `iter` / `list_all` | 自动翻页读取大目录 |
| `upload` / `download` | 本机 ↔ NAS。目标文件夹不存在会建 |
| `pull` | 让 NAS 去拉 HTTP，大文件优先用这个 |
| `move` / `copy` / `upload` | 默认覆盖同名，`force=False` 跳过 |
| `space` / `free` | 卷容量 / 路径剩余 |
| `size` | 文件或文件夹占用（属性窗口那套，会轮询到算完） |

`upload` 先通过 `fileUpload` 做空文件预检，再分块交给 `fileUploadV2`。失败重试前会查询
NAS 已确认偏移。默认覆盖同名文件，`force=False` 可跳过；下载使用可续传的 `.part` 文件。

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

结构化异常从 `ugnas` 直接导入：`ApiError`、`AuthExpired`、`NetworkError`、
`OperationTimeout`、`TransferError`、`TransferCancelled`，共同基类为 `UgnasError`。

## 分享与下载地址

文件外链，不是 SMB 共享文件夹。三种地址不要混用。

| 地址 | 过期 | 处理 |
| --- | --- | --- |
| 分享页 `link` | `expiration_days=0` 长期，直到取消或次数用尽 | 入库、发给别人 |
| 直链 `direct.url`（含 `task_id`） | 一次性，用完失效 | 不要缓存；要下再 `share_direct`，或 `download_share` 签完就拉 |
| `nas.download` 的 `dl_token` | 短时令牌 | 库内每次现取现拉，不要存 URL |

```python
info = nas.share_file("/volume1/share/tmp/c.zip")
print(info["link"])              # 长期保存这个
nas.share_direct(info["link"])   # 现场签发直链
nas.download_share(info["link"], r"D:\tmp\c.zip")
nas.unshare(info["share_id"])
```

## 命令行

```powershell
ugnas login
ugnas upload --path D:\tmp\a.zip --dest /volume1/share --progress
ugnas download --path /volume1/share/a.zip --dest D:\tmp\a.zip
ugnas pull --url http://example/file.zip --wait
ugnas rename --path /volume1/share/a.zip --name b.zip
ugnas move --path /volume1/share/b.zip --dest /volume1/share/tmp
ugnas share --path /volume1/share/tmp/b.zip
ugnas share-direct --url "http://ug.link/xxx/filemgr/share-download/?id=..."
ugnas download-share --url "http://ug.link/xxx/filemgr/share-download/?id=..." --dest D:\tmp\b.zip
ugnas unshare --share-id <id>
ugnas space
ugnas size --path /volume1/share
```
