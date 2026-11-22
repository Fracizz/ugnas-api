# unas-cli

用 Rust 管理绿联 UGOS Pro NAS，支持 Windows 和 Linux。编译后的命令可直接运行，无需安装 Python。

## 构建与安装

在仓库根目录使用 Rust 1.89 或更新版本。

```sh
cargo build --release --locked --manifest-path unas-cli/Cargo.toml
cargo install --locked --path unas-cli
```

构建产物位于 `unas-cli/target/release/`，文件名为 `unas-cli`（Linux）或 `unas-cli.exe`（Windows）。在目标平台构建对应的版本。

## 连接 NAS

```sh
unas-cli --profile home config save --base-url http://192.168.1.20:9999 --username myuser
unas-cli --profile home login --prompt-password
unas-cli --profile home roots
unas-cli --profile home ls --path /volume1/share --all --json
```

配置和会话放在 `~/.unas-cli/profiles/<profile>/`。设置 `UNAS_HOME` 可更改配置根目录。设置取值优先级为命令参数、环境变量、已保存配置。可用的环境变量有 `UNAS_PROFILE`、`UNAS_BASE_URL`、`UNAS_USERNAME`、`UNAS_PUBLIC_BASE_URL`、`UNAS_SAVE_DIR` 和 `UNAS_INSECURE`，也可使用对应的 `UGNAS_*` 变量。

密码可通过 `--prompt-password`、`--password-stdin` 或 `UNAS_PASSWORD` 提供。CLI 保存 NAS 会话，Windows 使用当前用户的 DPAPI 保护令牌，Unix 上会话文件仅当前用户可读写。用 `logout` 清除本地会话，或用 `--no-session` 执行一次性命令。HTTPS 默认验证证书，自签名设备可显式指定 `--insecure`。

## 常用命令

```sh
unas-cli --profile home mkdir --path /volume1/share/inbox
unas-cli --profile home upload --path ./archive.zip --dest /volume1/share/inbox --progress
unas-cli --profile home download --path /volume1/share/inbox/archive.zip --dest ./archive.zip --progress
unas-cli --profile home pull --url https://example.com/archive.zip --dest /volume1/share/inbox --wait
unas-cli --profile home share --path /volume1/share/inbox/archive.zip --days 7
unas-cli download-share --url 'https://example.ug.link/filemgr/share-download/?id=ID' --dest ./archive.zip
```

- `upload --dest` 是 NAS 目录，可用 `--name` 指定远端文件名。
- `download --dest` 和 `download-share --dest` 接受本地文件路径、已存在的目录或以斜杠结尾的目录路径。下载到已存在的文件需指定 `--overwrite`。服务端提供文件验证标识时，可在中断后继续下载。
- `copy` 和 `move` 会等待新目标路径出现。目标已经存在时返回 `submitted`，表示 NAS 已接受请求。
- 带口令的分享可用 `--prompt-share-password`、`--share-password-stdin` 或 `UNAS_SHARE_PASSWORD`。自定义分享域名需要显式配置 `--public-base-url`。
- `share-direct` 可生成一次性直链。输出默认隐藏 URL 中的令牌和密码，使用 `--show-sensitive` 获取完整链接。
- 用 `unas-cli --help` 查看命令，用 `unas-cli <命令> --help` 查看参数。

## 脚本调用

`--json` 将结果作为一个 JSON 文档写入标准输出。错误、密码提示和传输进度走标准错误。

| 退出码 | 含义 |
|---|---|
| 0 | 成功 |
| 1 | `exists` 查询的路径不存在 |
| 2 | 参数或配置错误 |
| 3 | 认证失败 |
| 4 | 网络错误 |
| 5 | NAS / HTTP API 错误 |
| 6 | 本地 I/O 或响应处理错误 |
| 7 | 等待 NAS 异步任务超时 |
