# ugnas-api

**English** · [简体中文](README.zh-CN.md)

Automate your UGREEN NAS from Python or the terminal. List files, resume uploads and downloads, ask the NAS to fetch a URL, or create a share. The client uses the local UGOS Pro API.

[Install](#installation) · [Quick start](#quick-start) · [Usage guide](docs/usage.en.md) · [Changelog](CHANGELOG.md) · [Security](SECURITY.md)

> **Disclaimer:** This project is independent and is not affiliated with or
> endorsed by UGREEN. The API is undocumented and may change between firmware
> releases.

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

Load real passwords from an environment variable, a local `.env` file, or a secret store. See [login and profile configuration](docs/usage.en.md#configure-login).

## Highlights

- RSA-protected login with reusable, profile-isolated sessions.
- File listing, creation, moves, copies, renames, deletion, uploads, and downloads.
- Resumable uploads and downloads with progress and cancellation hooks.
- NAS-side HTTP downloads for large files.
- External share creation, one-shot direct URLs, and safe shared-file downloads.
- Volume capacity, path free-space, and asynchronous directory-size queries.
- Python API and the `ugnas` command-line interface.
- Standalone Rust CLI. [Usage](unas-cli/README.md) · [Agent skill](skills/unas-cli/SKILL.md)

## Choose your interface

- **Python package** for Python automation. The [usage guide](docs/usage.en.md) covers account profiles, resumable transfers, NAS-side downloads, sharing, and capacity queries.
- **Python CLI** for shell use. Use `ugnas` and see the [command reference](docs/usage.en.md#cli-reference) for downloads, uploads, shares, and file operations.
- **Standalone Rust CLI** for using `unas-cli` without a Python environment. See the [download and usage guide](unas-cli/README.md) and [Agent Skill](skills/unas-cli/SKILL.md).

[Protocol documentation](docs/README.md) · [Security and redaction](SECURITY.md)

## License

MIT. See [LICENSE](LICENSE).
