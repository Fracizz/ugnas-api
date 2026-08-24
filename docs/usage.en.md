# Usage guide

`ugnas-api` is a Python client for the local JSON API exposed by UGOS Pro /
UGREEN NAS devices. Import the high-level facade with:

```python
from ugnas import Ugnas
```

The client is unofficial. Keep protocol assumptions isolated and test against
the firmware version used by your NAS.

## Install

```powershell
python -m pip install "ugnas-api @ git+https://github.com/Fracizz/ugnas-api.git"
```

For a checkout:

```powershell
python -m pip install -e .
```

The normal LAN API endpoint is `http://<NAS-IP>:9999/ugreen/`. Ports 80 and
443 generally serve the web UI. Use `insecure=True` only for plain HTTP or a
trusted self-signed certificate.

## Configure login

Credentials are resolved in this order:

1. Arguments passed to `Ugnas.connect()`.
2. Environment variables or a local `.env`.
3. The selected profile under `~/.ugnas/profiles/<profile>/`.

```python
import os

nas = Ugnas.connect(
    base_url=os.environ["UGNAS_BASE_URL"],
    username=os.environ["UGNAS_USERNAME"],
    password=os.environ["UGNAS_PASSWORD"],
    save_dir=os.environ.get("UGNAS_SAVE_DIR"),
    insecure=os.environ.get("UGNAS_INSECURE") == "1",
)
```

For CLI users:

```powershell
Copy-Item .env.example .env
# Fill in the local values; do not commit .env.
ugnas config-save --base-url http://192.168.x.x:9999 `
  --username local_user --save-dir "/volume1/share/downloads" --insecure
ugnas login --prompt-password
```

Supported variables include `UGNAS_BASE_URL`, `UGNAS_USERNAME`,
`UGNAS_PASSWORD`, `UGNAS_SAVE_DIR`, `UGNAS_PUBLIC_BASE_URL`,
`UGNAS_INSECURE`, and `UGNAS_PROFILE`. Use `--profile work` to isolate
multiple NAS accounts.

The password is used for login and is not written to the session file.
Successful logins cache a reusable session; expired sessions are refreshed or
re-authenticated automatically. Windows profile passwords and session tokens
are protected with DPAPI. Prefer `--prompt-password` or `--password-stdin`
over putting secrets in process arguments.

## File operations

Use forward slashes for NAS paths:

```python
nas.list("/volume1/share")
nas.exists("/volume1/share/archive.zip")
nas.mkdir("/volume1/share/incoming")

nas.upload(r"C:\tmp\a.zip", "/volume1/share")
nas.upload_to_path(r"C:\tmp\a.zip", "/volume1/share/incoming/a.zip")
nas.download("/volume1/share/incoming/a.zip", r"C:\tmp\a.zip")

nas.rename("/volume1/share/incoming/a.zip", "b.zip")
nas.move("/volume1/share/incoming/b.zip", "/volume1/share/archive")
nas.copy("/volume1/share/archive/b.zip", "/volume1/share/backup")
nas.delete(["/volume1/share/backup/b.zip"])
```

`move`, `copy`, and `upload` overwrite same-named targets by default. Pass
`force=False` to skip an existing target. `iter()` and `list_all()` paginate
large directories:

```python
for item in nas.iter("/volume1/share", page_size=200):
    print(item)
```

Uploads use the UGOS two-stage protocol and resume from a server-confirmed
offset after a recoverable interruption. Downloads use a validated `.part`
file and replace the destination atomically after the transfer completes.
Progress callbacks receive `(bytes_done, total_bytes_or_none)` and
`cancelled` callbacks can stop a transfer.

## NAS-side downloads

For multi-gigabyte HTTP files, let the NAS download center fetch the URL:

```python
nas.pull("https://example.com/archive.zip", wait=True)
```

The destination defaults to the configured `save_dir`; pass a directory
explicitly when needed.

## Shares and direct URLs

UGOS external shares are file links, not SMB shared folders:

```python
info = nas.share_file("/volume1/share/archive/b.zip")
print(info["link"])  # share page; suitable for saving or sending

nas.download_share(info["link"], r"C:\tmp\b.zip")
nas.unshare(info["share_id"])
```

Keep the share page URL or `share_id`, not a generated direct URL:

- `share_file()` returns a share page that can remain valid until revoked or
  its visit limit is exhausted.
- `share_direct()` signs a short-lived, one-use download URL.
- `download_share()` signs and downloads immediately, avoiding stale URLs.

## Capacity and errors

```python
print(nas.space())                 # volume totals, used, and free bytes
print(nas.free("/volume1/share"))  # free bytes for a path
print(nas.size("/volume1/share"))  # asynchronous directory size
```

The package exports `UgnasError` and more specific exceptions:
`ApiError`, `AuthExpired`, `NetworkError`, `OperationTimeout`,
`TransferError`, and `TransferCancelled`.

## CLI reference

```powershell
ugnas login --prompt-password
ugnas upload --path C:\tmp\a.zip --dest /volume1/share --progress
ugnas download --path /volume1/share/a.zip --dest C:\tmp\a.zip
ugnas pull --url https://example.com/file.zip --wait
ugnas share --path /volume1/share/a.zip
ugnas download-share --url "https://ug.link/example/..." --dest C:\tmp\a.zip
ugnas space
ugnas size --path /volume1/share
```

CLI JSON output redacts passwords, tokens, cookies, and one-shot URL
parameters by default. Use `--show-sensitive` only when output cannot be
logged or shared.

## Development

```powershell
uv sync --extra dev
uv run python -m unittest discover -s tests -v
uv run ruff check src tests
uv run mypy src/ugnas
uv build --clear
```

The CI matrix covers Python 3.10, 3.11, and 3.12 on Ubuntu and Windows.
Protocol observations are documented in
[api-login-filelist.md](api-login-filelist.md) and
[api-login-download-share.md](api-login-download-share.md).
