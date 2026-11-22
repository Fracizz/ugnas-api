# unas-cli

A standalone Rust command-line client for UGREEN UGOS Pro NAS devices. The Windows and Linux x86_64 binaries run without Python. The Linux build is statically linked. [Downloads](https://github.com/Fracizz/ugnas-api/releases?q=unas-cli)

## Install

Extract the archive and run `./unas-cli --help` on Linux or `.\unas-cli.exe --help` in PowerShell. Verify the archive against the release's `SHA256SUMS` file.

From the repository root, build or install with Rust 1.89 or later.

```sh
cargo build --release --locked --manifest-path unas-cli/Cargo.toml
cargo install --locked --path unas-cli
```

To install the agent skill, extract the skill ZIP into your agent's skills directory. The extracted folder contains `unas-cli/SKILL.md`. [Skill source](https://github.com/Fracizz/ugnas-api/blob/master/skills/unas-cli/SKILL.md)

## Connect

```sh
unas-cli --profile home config save --base-url http://192.168.1.20:9999 --username myuser
unas-cli --profile home login --prompt-password
unas-cli --profile home roots
unas-cli --profile home ls --path /volume1/share --all --json
```

Profiles and sessions live in `~/.unas-cli/profiles/<profile>/`. Set `UNAS_HOME` to override the root. Flags override environment variables, which override saved settings. Supported variables include `UNAS_PROFILE`, `UNAS_BASE_URL`, `UNAS_USERNAME`, `UNAS_PUBLIC_BASE_URL`, `UNAS_SAVE_DIR`, and `UNAS_INSECURE`. Corresponding `UGNAS_*` variables are also supported.

Supply the NAS password with `--prompt-password`, `--password-stdin`, or `UNAS_PASSWORD`. Windows protects saved session tokens with DPAPI. On Unix, session files are readable and writable only by the current user. Use `logout` to clear a local session or `--no-session` to skip the session cache. HTTPS verifies certificates by default. Use `--insecure` for a trusted device with a self-signed certificate.

## Common commands

```sh
unas-cli --profile home mkdir --path /volume1/share/inbox
unas-cli --profile home upload --path ./archive.zip --dest /volume1/share/inbox --progress
unas-cli --profile home download --path /volume1/share/inbox/archive.zip --dest ./archive.zip --progress
unas-cli --profile home pull --url https://example.com/archive.zip --dest /volume1/share/inbox --wait
unas-cli --profile home share --path /volume1/share/inbox/archive.zip --days 7
unas-cli download-share --url 'https://example.ug.link/filemgr/share-download/?id=ID' --dest ./archive.zip
```

- Upload destinations are NAS directories. Use `--name` to set the remote filename.
- A download destination can be a file path, an existing directory, or a directory path ending in a slash. Use `--overwrite` to replace a local file. Downloads resume when the server provides a validator. Use `--dry-run` to probe the download.
- Copy and move wait for new target paths to appear. A pre-existing target returns `submitted` when the NAS accepts the request.
- For password-protected shares, use `--prompt-share-password`, `--share-password-stdin`, or `UNAS_SHARE_PASSWORD`. Configure `--public-base-url` for custom share hosts. `share-direct` redacts its short-lived link by default. Use `--show-sensitive` to get the complete link.
- Run `unas-cli <command> --help` for all options.

## Automation

`--json` writes one JSON document to stdout. Errors, prompts, and progress go to stderr.

| Exit code | Meaning |
|---|---|
| 0 | Success |
| 1 | `exists` found no path |
| 2 | Invalid arguments or configuration |
| 3 | Authentication failure |
| 4 | Network error |
| 5 | NAS or HTTP API error |
| 6 | Local I/O or response processing error |
| 7 | NAS task timeout |
