mod client;
mod config;
mod ops;
mod transfer;

use clap::{Args, Parser, Subcommand};
use client::{Error, NasClient, Result};
use config::{Profile, Settings, env_value};
use reqwest::Url;
use serde_json::{Value, json};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use transfer::DownloadOptions;

#[derive(Parser, Debug)]
#[command(version, about = "Manage files on a UGREEN UGOS Pro NAS")]
struct Cli {
    #[command(flatten)]
    options: Options,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Debug)]
struct Options {
    /// Select a saved NAS profile
    #[arg(long, global = true)]
    profile: Option<String>,
    /// NAS API base URL
    #[arg(long, global = true)]
    base_url: Option<String>,
    /// NAS username
    #[arg(long, global = true)]
    username: Option<String>,
    /// Override the public share API base URL
    #[arg(long, global = true)]
    public_base_url: Option<String>,
    /// Default NAS output directory for pull
    #[arg(long, global = true)]
    save_dir: Option<String>,
    /// Accept invalid TLS certificates
    #[arg(long, global = true, conflicts_with = "secure")]
    insecure: bool,
    /// Require valid TLS certificates, overriding profile settings
    #[arg(long, global = true)]
    secure: bool,
    /// Read the NAS password from the first line of stdin
    #[arg(long, global = true, conflicts_with = "prompt_password")]
    password_stdin: bool,
    /// Prompt for a NAS password without echoing it
    #[arg(long, global = true)]
    prompt_password: bool,
    /// Run without reading or writing a saved login session
    #[arg(long, global = true)]
    no_session: bool,
    /// Write one JSON document to stdout
    #[arg(long, global = true)]
    json: bool,
    /// Include sensitive response fields, such as direct link tokens
    #[arg(long, global = true)]
    show_sensitive: bool,
}

impl Options {
    fn settings(&self) -> Settings {
        Settings {
            base_url: self.base_url.clone(),
            username: self.username.clone(),
            public_base_url: self.public_base_url.clone(),
            save_dir: self.save_dir.clone(),
            insecure: if self.insecure {
                Some(true)
            } else if self.secure {
                Some(false)
            } else {
                None
            },
        }
    }

    fn password(&self) -> Result<Option<String>> {
        read_secret(
            self.password_stdin,
            self.prompt_password,
            "PASSWORD",
            "NAS password: ",
        )
    }
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Save or inspect a NAS profile
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Log in and save or check a NAS session
    Login,
    /// Remove this profile's local login session
    Logout,
    /// List the NAS root directories
    Roots,
    /// List the contents of a NAS directory
    #[command(visible_alias = "list")]
    Ls {
        #[arg(long)]
        path: String,
        #[command(flatten)]
        page: Page,
        /// Read all remaining pages
        #[arg(long)]
        all: bool,
    },
    /// Check whether a NAS path exists
    Exists {
        #[arg(long)]
        path: String,
    },
    /// Create a NAS directory, including missing parents
    Mkdir {
        #[arg(long)]
        path: String,
    },
    /// Delete one or more NAS paths
    #[command(visible_alias = "rm")]
    Delete {
        #[command(flatten)]
        paths: Paths,
    },
    /// Rename a file or directory
    Rename {
        #[arg(long)]
        path: String,
        #[arg(long)]
        name: String,
    },
    /// Move files or directories to a NAS directory
    Move(Relocate),
    /// Copy files or directories to a NAS directory
    Copy(Relocate),
    /// Query volume capacity or free space at a specific path
    Space {
        #[arg(long)]
        path: Option<String>,
    },
    /// Query the size and item count of a file or directory
    Size {
        #[arg(long)]
        path: String,
        #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,
    },
    /// Submit a URL for the NAS to download
    Pull {
        #[arg(long)]
        url: String,
        /// Override the default NAS output directory
        #[arg(long)]
        dest: Option<String>,
        #[arg(long, visible_alias = "force")]
        overwrite: bool,
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,
    },
    /// List NAS download tasks
    Downloads {
        /// Show completed tasks
        #[arg(long)]
        completed: bool,
    },
    /// Upload a local file to a NAS directory
    Upload {
        #[arg(long)]
        path: PathBuf,
        /// Destination directory on the NAS
        #[arg(long)]
        dest: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, visible_alias = "force")]
        overwrite: bool,
        #[arg(long)]
        progress: bool,
    },
    /// Download a NAS file to the local filesystem
    Download {
        #[arg(long)]
        path: String,
        #[command(flatten)]
        file: DownloadFile,
    },
    /// Create a public share link
    Share {
        #[command(flatten)]
        paths: Paths,
        /// Days until expiry, or 0 for no expiry
        #[arg(long, default_value_t = 0)]
        days: u32,
        /// Also request a short-lived direct download link
        #[arg(long)]
        direct: bool,
        #[command(flatten)]
        secret: ShareSecret,
    },
    /// List public shares created by the account
    Shares {
        #[arg(long)]
        keyword: Option<String>,
        #[command(flatten)]
        page: Page,
    },
    /// Revoke one or more shares
    Unshare {
        /// Share ID or link, repeat for multiple shares
        #[arg(long, required = true)]
        share_id: Vec<String>,
    },
    /// Request a short-lived direct download link for a share
    ShareDirect(ShareAccess),
    /// Download a file from a public share
    DownloadShare {
        #[command(flatten)]
        access: ShareAccess,
        #[command(flatten)]
        file: DownloadFile,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    /// Save settings supplied on the command line
    Save,
    /// Print the effective settings for the selected profile
    Show,
}

#[derive(Args, Debug)]
struct Paths {
    /// NAS path, repeat for multiple paths
    #[arg(long = "path", required = true)]
    values: Vec<String>,
}

#[derive(Args, Debug)]
struct Relocate {
    #[command(flatten)]
    paths: Paths,
    #[arg(long)]
    dest: String,
    #[arg(long, visible_alias = "force")]
    overwrite: bool,
    #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..))]
    timeout: u64,
}

#[derive(Args, Debug)]
struct Page {
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    page: u32,
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..))]
    limit: u32,
}

#[derive(Args, Debug)]
struct ShareAccess {
    /// Public share page URL
    #[arg(
        long,
        required_unless_present = "share_id",
        conflicts_with = "share_id"
    )]
    url: Option<String>,
    /// Public share ID
    #[arg(long)]
    share_id: Option<String>,
    /// NAS path within the share, repeat to select multiple paths
    #[arg(long)]
    path: Vec<String>,
    #[command(flatten)]
    secret: ShareSecret,
}

impl ShareAccess {
    fn reference(&self) -> Result<&str> {
        self.url
            .as_deref()
            .or(self.share_id.as_deref())
            .ok_or_else(|| Error::Usage("provide --url or --share-id".into()))
    }
}

#[derive(Args, Debug)]
struct ShareSecret {
    /// Read the share password from the first line of stdin
    #[arg(long, conflicts_with = "prompt_share_password")]
    share_password_stdin: bool,
    /// Prompt for the share password without echoing it
    #[arg(long)]
    prompt_share_password: bool,
}

impl ShareSecret {
    fn validate(&self, options: &Options) -> Result<()> {
        if self.share_password_stdin && options.password_stdin {
            return Err(Error::Usage(
                "use stdin for either the NAS password or the share password".into(),
            ));
        }
        Ok(())
    }

    fn read(&self) -> Result<Option<String>> {
        read_secret(
            self.share_password_stdin,
            self.prompt_share_password,
            "SHARE_PASSWORD",
            "Share password: ",
        )
    }
}

#[derive(Args, Debug)]
struct DownloadFile {
    /// Exact local file path, or a directory with a trailing slash
    #[arg(long)]
    dest: PathBuf,
    /// Check the response without writing a local file
    #[arg(long)]
    dry_run: bool,
    /// Replace an existing local file once download completes
    #[arg(long, visible_alias = "force")]
    overwrite: bool,
    #[arg(long)]
    progress: bool,
}

impl DownloadFile {
    fn options(&self) -> DownloadOptions<'_> {
        DownloadOptions {
            dest: &self.dest,
            dry_run: self.dry_run,
            overwrite: self.overwrite,
            progress: self.progress,
        }
    }
}

fn read_secret(from_stdin: bool, prompt: bool, env: &str, label: &str) -> Result<Option<String>> {
    if from_stdin {
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        line.truncate(line.trim_end_matches(['\r', '\n']).len());
        if line.is_empty() {
            return Err(Error::Usage(format!("no {env} on stdin")));
        }
        return Ok(Some(line));
    }
    if prompt {
        return rpassword::prompt_password(label)
            .map(Some)
            .map_err(Into::into);
    }
    Ok(env_value(env))
}

fn authenticated(options: &Options, settings: &Settings, profile: &Profile) -> Result<NasClient> {
    let base = settings
        .base_url
        .as_deref()
        .ok_or_else(|| Error::Usage("set --base-url or UNAS_BASE_URL first".into()))?;
    let username = settings
        .username
        .as_deref()
        .ok_or_else(|| Error::Usage("set --username or UNAS_USERNAME first".into()))?;
    let password = options.password()?;
    let session = if options.no_session {
        None
    } else {
        profile.ensure_dir()?;
        Some(profile.session_path())
    };
    let mut client = NasClient::new(base, settings.insecure.unwrap_or(false), session)?;
    client.connect(username, password.as_deref())?;
    Ok(client)
}

fn public_share(
    options: &Options,
    settings: &Settings,
    profile: &Profile,
    access: &ShareAccess,
    download: Option<&DownloadFile>,
) -> Result<Value> {
    let reference = access.reference()?;
    let (_, origin) = ops::share_reference(reference)?;
    let base = settings.public_base_url.as_deref().or(origin.as_deref());
    let mut client = if let Some(base) = base {
        NasClient::new(base, settings.insecure.unwrap_or(false), None)?
    } else if reference.contains("://") {
        return Err(Error::Usage(
            "set --public-base-url for a custom share host".into(),
        ));
    } else {
        authenticated(options, settings, profile)?
    };
    let password = access.secret.read()?.unwrap_or_default();
    let sign = |client: &mut NasClient| {
        ops::share_direct(
            client,
            reference,
            &password,
            &access.path,
            settings.public_base_url.as_deref(),
        )
    };
    let direct = sign(&mut client)?;
    let Some(file) = download else {
        return Ok(direct);
    };
    let stream = |client: &NasClient, direct: &Value| {
        let url = direct["url"]
            .as_str()
            .ok_or_else(|| Error::Other("share response contains no download URL".into()))?;
        let identity = json!({"share_id": direct["share_id"], "paths": direct["paths"]});
        transfer::download_public(
            client,
            url,
            &identity.to_string(),
            direct["name"].as_str(),
            file.options(),
        )
    };
    match stream(&client, &direct) {
        Err(error) if error.status() == Some(401) => {
            let fresh = sign(&mut client)?;
            stream(&client, &fresh)
        }
        result => result,
    }
}

fn run_nas(
    client: &mut NasClient,
    command: &Command,
    default_save_dir: Option<&str>,
    public_base: Option<&str>,
    share_password: Option<&str>,
) -> Result<Value> {
    match command {
        Command::Login => Ok(json!({"status": "ok", "reused_session": client.reused_session()})),
        Command::Roots => ops::roots(client),
        Command::Ls { path, page, all } => ops::list(client, path, page.page, page.limit, *all),
        Command::Exists { path } => Ok(json!({"path": path, "exists": ops::exists(client, path)?})),
        Command::Mkdir { path } => {
            ops::mkdir(client, path)?;
            Ok(json!({"path": path, "status": "ok"}))
        }
        Command::Delete { paths } => ops::delete(client, &paths.values),
        Command::Rename { path, name } => ops::rename(client, path, name),
        Command::Move(args) | Command::Copy(args) => ops::copy_or_move(
            client,
            &args.paths.values,
            &args.dest,
            matches!(command, Command::Move(_)),
            args.overwrite,
            Duration::from_secs(args.timeout),
        ),
        Command::Space { path } => ops::space(client, path.as_deref()),
        Command::Size { path, timeout } => ops::size(client, path, Duration::from_secs(*timeout)),
        Command::Pull {
            url,
            dest,
            overwrite,
            wait,
            timeout,
        } => {
            let dest = dest.as_deref().or(default_save_dir).ok_or_else(|| {
                Error::Usage("pull requires --dest or a configured --save-dir".into())
            })?;
            ops::pull(
                client,
                url,
                dest,
                *overwrite,
                *wait,
                Duration::from_secs(*timeout),
            )
        }
        Command::Downloads { completed } => ops::downloads(client, *completed),
        Command::Upload {
            path,
            dest,
            name,
            overwrite,
            progress,
        } => transfer::upload(client, path, dest, name.as_deref(), *overwrite, *progress),
        Command::Download { path, file } => transfer::download(client, path, file.options()),
        Command::Share {
            paths,
            days,
            direct,
            ..
        } => {
            let mut share = ops::share(client, &paths.values, share_password, *days, public_base)?;
            if *direct {
                let page = share["link"].as_str().filter(|url| !url.is_empty());
                let known_page = page.filter(|url| {
                    ops::share_reference(url).is_ok_and(|(_, origin)| origin.is_some())
                });
                let link = known_page
                    .or_else(|| share["share_id"].as_str().filter(|id| !id.is_empty()))
                    .or(page)
                    .ok_or_else(|| {
                        Error::Other("share was created without a public ID or link".into())
                    })?;
                share["direct"] = ops::share_direct(
                    client,
                    link,
                    share_password.unwrap_or_default(),
                    &paths.values,
                    public_base,
                )?;
            }
            Ok(share)
        }
        Command::Shares { keyword, page } => {
            ops::list_shares(client, keyword.as_deref(), page.page, page.limit)
        }
        Command::Unshare { share_id } => ops::unshare(client, share_id),
        Command::Config { .. }
        | Command::Logout
        | Command::ShareDirect(_)
        | Command::DownloadShare { .. } => unreachable!("handled before authentication"),
    }
}

fn run(cli: &Cli) -> Result<Value> {
    let options = &cli.options;
    match &cli.command {
        Command::Share { secret, .. } => secret.validate(options)?,
        Command::ShareDirect(access) | Command::DownloadShare { access, .. } => {
            access.secret.validate(options)?;
        }
        _ => {}
    }
    let profile = Profile::resolve(options.profile.as_deref())?;
    match &cli.command {
        Command::Config {
            command: ConfigCommand::Save,
        } => {
            let file = profile.save(options.settings())?;
            Ok(json!({"config": file.to_string_lossy(), "status": "saved"}))
        }
        Command::Logout => {
            profile.clear_session()?;
            Ok(json!({"status": "ok"}))
        }
        command => {
            let settings = profile.effective(options.settings())?;
            match command {
                Command::Config {
                    command: ConfigCommand::Show,
                } => Ok(json!(settings)),
                Command::ShareDirect(access) => {
                    public_share(options, &settings, &profile, access, None)
                }
                Command::DownloadShare { access, file } => {
                    public_share(options, &settings, &profile, access, Some(file))
                }
                _ => {
                    if let Command::Pull { dest: None, .. } = command
                        && settings.save_dir.is_none()
                    {
                        return Err(Error::Usage(
                            "pull requires --dest or a configured --save-dir".into(),
                        ));
                    }
                    let mut client = authenticated(options, &settings, &profile)?;
                    let password = if let Command::Share { secret, .. } = command {
                        secret.read()?
                    } else {
                        None
                    };
                    run_nas(
                        &mut client,
                        command,
                        settings.save_dir.as_deref(),
                        settings.public_base_url.as_deref(),
                        password.as_deref(),
                    )
                }
            }
        }
    }
}

fn sensitive_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    normalized.ends_with("token")
        || normalized.ends_with("password")
        || normalized.ends_with("secret")
        || matches!(
            normalized.as_str(),
            "authorization" | "cookie" | "setcookie" | "signature" | "sign" | "taskid" | "passcode"
        )
}

fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                if sensitive_key(key) {
                    *item = Value::String("<redacted>".into());
                } else {
                    redact(item);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        Value::String(text) if text.starts_with("http://") || text.starts_with("https://") => {
            let Ok(mut url) = Url::parse(text) else {
                return;
            };
            let mut dirty = !url.username().is_empty() || url.password().is_some();
            if dirty {
                let _ = url.set_username("");
                let _ = url.set_password(None);
            }
            if url.query_pairs().any(|(key, _)| sensitive_key(&key)) {
                let pairs: Vec<_> = url
                    .query_pairs()
                    .map(|(key, val)| {
                        let val = if sensitive_key(&key) {
                            "<redacted>".into()
                        } else {
                            val.into_owned()
                        };
                        (key.into_owned(), val)
                    })
                    .collect();
                url.query_pairs_mut().clear().extend_pairs(pairs);
                dirty = true;
            }
            if dirty {
                *text = url.into();
            }
        }
        _ => {}
    }
}

fn exists_exit(command: &Command, value: &Value) -> ExitCode {
    if matches!(command, Command::Exists { .. }) && value["exists"] == false {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn render(writer: &mut impl Write, cli: &Cli, value: &Value) -> io::Result<()> {
    if !cli.options.json {
        match &cli.command {
            Command::Ls { .. } | Command::Roots if value["items"].is_array() => {
                if let Some(items) = value["items"].as_array() {
                    for item in items {
                        if let Some(name) = item["name"].as_str().or(item["path"].as_str()) {
                            for character in name.chars() {
                                if character.is_control() {
                                    write!(writer, "{}", character.escape_default())?;
                                } else {
                                    write!(writer, "{character}")?;
                                }
                            }
                            writeln!(writer)?;
                        } else {
                            writeln!(writer, "{item}")?;
                        }
                    }
                    return Ok(());
                }
            }
            Command::Exists { .. } => return writeln!(writer, "{}", value["exists"]),
            _ => {}
        }
    }
    serde_json::to_writer_pretty(&mut *writer, value).map_err(|error| {
        if let Some(kind) = error.io_error_kind() {
            io::Error::new(kind, error)
        } else {
            io::Error::other(error)
        }
    })?;
    writeln!(writer)
}

fn error_code(error: &Error) -> ExitCode {
    ExitCode::from(match error {
        Error::Usage(_) => 2,
        Error::Auth(_) => 3,
        Error::Network(_) => 4,
        Error::Api { .. } => 5,
        Error::Io(_) | Error::Other(_) => 6,
        Error::Timeout(_) => 7,
    })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(mut value) => {
            let status = exists_exit(&cli.command, &value);
            if !cli.options.show_sensitive {
                redact(&mut value);
            }
            let mut writer = BufWriter::new(io::stdout().lock());
            match render(&mut writer, &cli, &value).and_then(|()| writer.flush()) {
                Ok(()) => status,
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => status,
                Err(error) => {
                    eprintln!("error: {error}");
                    ExitCode::from(6)
                }
            }
        }
        Err(error) => {
            eprintln!("error: {error}");
            error_code(&error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_invalid_invocations_before_login() {
        assert!(Cli::try_parse_from(["unas-cli", "download"]).is_err());
        assert!(
            Cli::try_parse_from(["unas-cli", "ls", "--path", "/volume1", "--page", "0"]).is_err()
        );
        assert!(Cli::try_parse_from(["unas-cli", "share-direct"]).is_err());
        assert!(
            Cli::try_parse_from([
                "unas-cli",
                "share-direct",
                "--url",
                "https://nas.ug.link/?id=a",
                "--share-id",
                "a"
            ])
            .is_err()
        );
        for args in [
            [
                "unas-cli",
                "--password-stdin",
                "share",
                "--path",
                "/file",
                "--share-password-stdin",
            ],
            [
                "unas-cli",
                "share",
                "--path",
                "/file",
                "--share-password-stdin",
                "--password-stdin",
            ],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(matches!(run(&cli), Err(Error::Usage(_))));
        }
    }

    #[test]
    fn redacts_nested_credentials_and_download_query_tokens() {
        let mut response = json!({
            "task_id": "private",
            "link": "https://user:pw@nas.example/share?id=public&dl_token=private",
            "files": [{"X-Ugreen-Token": "private", "name": "报告.txt"}]
        });
        redact(&mut response);
        let text = response.to_string();
        assert!(!text.contains("private"));
        assert!(!text.contains("user:pw"));
        assert!(text.contains("id=public"));
        assert!(text.contains("报告.txt"));
    }

    #[test]
    fn renders_json_and_reports_missing_path_with_status_one() {
        let cli =
            Cli::try_parse_from(["unas-cli", "exists", "--path", "/missing", "--json"]).unwrap();
        let value = json!({"path": "/missing", "exists": false});
        let mut output = Vec::new();
        render(&mut output, &cli, &value).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&output).unwrap(), value);
        assert_eq!(exists_exit(&cli.command, &value), ExitCode::from(1));
    }
}
