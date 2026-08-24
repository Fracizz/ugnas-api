# ugnas-api

绿联云（UGOS Pro）/ UGREEN NAS 的非官方 Python 客户端。项目提供登录、文件管理、NAS 下载中心、外链分享、容量查询，以及支持断点续传的上传和下载能力。

> **免责声明：** 本项目独立开发，与 UGREEN 没有隶属或背书关系。项目使用的是未公开的设备接口，接口可能随固件版本变化。

[English README](README.md) · [中文使用指南](docs/usage.md) · [变更记录](CHANGELOG.zh-CN.md) · [安全说明](SECURITY.md)

## 安装

从 GitHub 安装：

```powershell
python -m pip install "ugnas-api @ git+https://github.com/Fracizz/ugnas-api.git"
```

开发本仓库：

```powershell
python -m pip install -e .
```

## 快速开始

局域网 JSON API 通常是 `http://<NAS-IP>:9999`；80 和 443 通常是网页端口，不是 JSON API。

```python
from ugnas import Ugnas

nas = Ugnas.connect(
    base_url="http://192.168.x.x:9999",
    username="local_user",
    password="从安全存储读取",
    save_dir="/volume1/share/downloads",
    insecure=True,  # 仅用于 HTTP 或可信的自签名服务
)

print(nas.list("/volume1/share"))
nas.download("/volume1/share/archive.zip", r"C:\tmp\archive.zip")
```

不要把真实密码写入源代码。可使用 `.env`、环境变量或 profile 配置；Windows 上保存的密码和会话令牌使用当前用户 DPAPI 保护。

## 能力概览

- 登录、会话复用、令牌续期和多 profile 隔离。
- 列目录、创建、移动、复制、重命名、删除、上传和下载文件。
- 支持进度、取消和断点续传的文件传输。
- 使用 `pull()` 让 NAS 自己下载大文件。
- 创建外链、现场签发一次性直链，以及安全下载分享文件。
- 查询卷容量、路径剩余空间和异步目录占用。
- Python API 与 `ugnas` 命令行工具。

## 安全注意事项

密码、会话文件、token、Cookie、抓包文件和一次性直链都应视为敏感信息。CLI 默认会脱敏 URL 中的密码、token 和任务参数；只有确认输出不会被记录或分享时，才使用 `--show-sensitive`。

## 开发

```powershell
uv sync --extra dev
uv run python -m unittest discover -s tests -v
uv run ruff check src tests
uv run mypy src/ugnas
```

完整用法见 [docs/usage.md](docs/usage.md)，协议笔记见 [docs/](docs/)，许可证见 [LICENSE](LICENSE)。
