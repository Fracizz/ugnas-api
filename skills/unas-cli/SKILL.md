---
name: unas-cli
description: Manage a UGREEN UGOS Pro NAS with the unas-cli executable. Use for NAS file and directory operations, transfers, capacity checks, NAS-side downloads, and share management.
---

# unas-cli

Use `unas-cli --version` to check the binary. After extraction, run `./unas-cli` on Linux or `.\unas-cli.exe` in PowerShell. If needed, get the matching Windows or Linux x86_64 archive from [GitHub Releases](https://github.com/Fracizz/ugnas-api/releases?q=unas-cli) and extract it. Use `unas-cli <command> --help` for exact flags.

## Connect

Use the requested `--profile`. The default profile is `default`. Settings come from flags, `UNAS_*` environment variables, then the saved profile. Set `UNAS_HOME` to isolate profile files. Use the NAS API URL, typically `http://<NAS-IP>:9999` on a LAN.

```sh
unas-cli --profile home config save --base-url "http://NAS-IP:9999" --username "USERNAME"
unas-cli --profile home login --prompt-password
unas-cli --profile home ls --path "/volume1/share" --all --json
```

Reuse a saved session when available. Use `--password-stdin` or a secured `UNAS_PASSWORD` variable for unattended login. Keep passwords and session tokens out of output. Use `--no-session` for one-off commands and `logout` to clear a local session. HTTPS verifies certificates by default.

## Run

| Task | Command and key options |
|---|---|
| Browse or inspect | `roots`, `ls --path PATH [--all]`, `exists --path PATH`, `space [--path PATH]`, `size --path PATH` |
| Modify NAS files | `mkdir --path PATH`, `rename --path PATH --name NAME`, `copy` or `move` with `--path PATH --dest NAS_DIR`, `delete --path PATH` |
| Transfer a file | `upload --path LOCAL_FILE --dest NAS_DIR [--name NAME]`, `download --path NAS_FILE --dest LOCAL_PATH` |
| Download on the NAS | `pull --url URL --dest NAS_DIR [--wait]`, `downloads [--completed]` |
| Manage shares | `share --path PATH [--days N]`, `shares`, `unshare --share-id ID`, `share-direct --url URL`, `download-share --url URL --dest LOCAL_PATH` |

- NAS paths are absolute. For local downloads, an existing directory or a trailing slash selects a directory. Use `--dry-run` on downloads to probe the response. Pass `--overwrite` when replacement is intended.
- If copy or move returns `submitted`, the target already existed and the NAS accepted the request. `pull` can use the profile's `--save-dir`. If `pull --wait` times out, check `downloads` before retrying.
- Shares default to no expiry. Use `--days` for an expiry. Supply a share password through `--share-password-stdin`, `--prompt-share-password`, or `UNAS_SHARE_PASSWORD`. Use `--url` or `--share-id` to access a share. Set `--public-base-url` for custom share hosts. Use `--show-sensitive` only when the user needs the complete short-lived direct link.

## Read results

Use `--json` for a single JSON document on stdout. Errors and optional `--progress` output go to stderr. Exit codes are `0` success, `1` missing path from `exists`, `2` arguments/config, `3` authentication, `4` network, `5` API, `6` local I/O or response processing, and `7` task timeout.
