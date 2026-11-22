use crate::client::{Error, Result};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, DirBuilder};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) public_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) save_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) insecure: Option<bool>,
}

impl Settings {
    fn overlay(&mut self, supplied: Self) {
        if supplied.base_url.is_some() {
            self.base_url = supplied.base_url;
        }
        if supplied.username.is_some() {
            self.username = supplied.username;
        }
        if supplied.public_base_url.is_some() {
            self.public_base_url = supplied.public_base_url;
        }
        if supplied.save_dir.is_some() {
            self.save_dir = supplied.save_dir;
        }
        if supplied.insecure.is_some() {
            self.insecure = supplied.insecure;
        }
    }

    fn validate(&self) -> Result<()> {
        for (label, value) in [
            ("base URL", self.base_url.as_deref()),
            ("public base URL", self.public_base_url.as_deref()),
        ] {
            if let Some(value) = value {
                let url =
                    Url::parse(value).map_err(|_| Error::Usage(format!("invalid {label}")))?;
                if !matches!(url.scheme(), "http" | "https")
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err(Error::Usage(format!(
                        "{label} must be an HTTP(S) URL without credentials, query, or fragment"
                    )));
                }
            }
        }
        if self
            .username
            .as_ref()
            .is_some_and(|user| user.trim().is_empty())
        {
            return Err(Error::Usage("username must not be empty".into()));
        }
        if self
            .save_dir
            .as_ref()
            .is_some_and(|dir| dir.trim().is_empty())
        {
            return Err(Error::Usage("save directory must not be empty".into()));
        }
        Ok(())
    }
}

pub(crate) struct Profile {
    dir: PathBuf,
}

impl Profile {
    pub(crate) fn resolve(explicit: Option<&str>) -> Result<Self> {
        let name = explicit
            .map(str::to_owned)
            .or_else(|| env_value("PROFILE"))
            .unwrap_or_else(|| "default".into());
        let home = if let Some(path) = env::var_os("UNAS_HOME").filter(|path| !path.is_empty()) {
            PathBuf::from(path)
        } else {
            let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
            let home = env::var_os(var)
                .filter(|path| !path.is_empty())
                .ok_or_else(|| Error::Usage(format!("set UNAS_HOME or {var}")))?;
            PathBuf::from(home).join(".unas-cli")
        };
        Self::in_dir(&home, &name)
    }

    fn in_dir(home: &Path, name: &str) -> Result<Self> {
        if name.is_empty()
            || name.len() > 64
            || !name.as_bytes()[0].is_ascii_alphanumeric()
            || !name.as_bytes()[name.len() - 1].is_ascii_alphanumeric()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(Error::Usage(
                "profile name must be 1–64 ASCII letters, digits, dots, underscores, or hyphens, and start and end with a letter or digit".into(),
            ));
        }
        let first = name
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if matches!(first.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || first
                .strip_prefix("COM")
                .or_else(|| first.strip_prefix("LPT"))
                .is_some_and(|port| port.len() == 1 && port.as_bytes()[0].is_ascii_digit())
        {
            return Err(Error::Usage("profile name is reserved on Windows".into()));
        }
        Ok(Self {
            dir: home.join("profiles").join(name),
        })
    }

    pub(crate) fn config_path(&self) -> PathBuf {
        self.dir.join("config.json")
    }

    pub(crate) fn session_path(&self) -> PathBuf {
        self.dir.join("session.json")
    }

    pub(crate) fn ensure_dir(&self) -> Result<()> {
        let mut builder = DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&self.dir)?;
        Ok(())
    }

    pub(crate) fn load(&self) -> Result<Settings> {
        let path = self.config_path();
        match fs::read(&path) {
            Ok(data) => serde_json::from_slice(&data).map_err(|error| {
                Error::Usage(format!(
                    "invalid profile config {} at line {}, column {}",
                    path.display(),
                    error.line(),
                    error.column()
                ))
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn effective(&self, args: Settings) -> Result<Settings> {
        let mut settings = self.load()?;
        settings.overlay(Settings {
            base_url: env_value("BASE_URL"),
            username: env_value("USERNAME"),
            public_base_url: env_value("PUBLIC_BASE_URL"),
            save_dir: env_value("SAVE_DIR"),
            insecure: if args.insecure.is_some() {
                None
            } else {
                env_value("INSECURE")
                    .as_deref()
                    .map(parse_bool)
                    .transpose()?
            },
        });
        settings.overlay(args);
        settings.validate()?;
        Ok(settings)
    }

    pub(crate) fn save(&self, changes: Settings) -> Result<PathBuf> {
        if changes == Settings::default() {
            return Err(Error::Usage("provide at least one setting to save".into()));
        }
        let mut settings = self.load()?;
        settings.overlay(changes);
        settings.validate()?;
        self.ensure_dir()?;
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        serde_json::to_writer_pretty(file.as_file_mut(), &settings)
            .map_err(|_| Error::Other("cannot write profile config".into()))?;
        writeln!(file)?;
        file.as_file().sync_all()?;
        let path = self.config_path();
        file.persist(&path).map_err(|err| err.error)?;
        Ok(path)
    }

    pub(crate) fn clear_session(&self) -> Result<()> {
        match fs::remove_file(self.session_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

pub(crate) fn env_value(suffix: &str) -> Option<String> {
    ["UNAS", "UGNAS"].into_iter().find_map(|prefix| {
        env::var(format!("{prefix}_{suffix}"))
            .ok()
            .filter(|value| !value.is_empty())
    })
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(Error::Usage("UNAS_INSECURE must be true or false".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_path_rejects_traversal_and_reserved_names() {
        for invalid in ["", ".", "..", "one/two", "two\\one", "NAS.", "NUL.json"] {
            assert!(Profile::in_dir(Path::new("test"), invalid).is_err());
        }
        assert!(Profile::in_dir(Path::new("test"), "home-nas_2").is_ok());
    }

    #[test]
    fn profile_saves_partial_updates_without_cross_profile_changes() {
        let home = tempfile::tempdir().unwrap();
        let first = Profile::in_dir(home.path(), "first").unwrap();
        let second = Profile::in_dir(home.path(), "second").unwrap();
        first
            .save(Settings {
                base_url: Some("https://nas.example".into()),
                insecure: Some(true),
                ..Settings::default()
            })
            .unwrap();
        first
            .save(Settings {
                username: Some("nas-user".into()),
                insecure: Some(false),
                ..Settings::default()
            })
            .unwrap();
        let settings = first.load().unwrap();
        assert_eq!(settings.base_url.as_deref(), Some("https://nas.example"));
        assert_eq!(settings.username.as_deref(), Some("nas-user"));
        assert_eq!(settings.insecure, Some(false));
        assert_eq!(second.load().unwrap(), Settings::default());
    }

    #[test]
    fn config_rejects_credentials_in_urls() {
        assert!(
            Settings {
                base_url: Some("https://account:secret@nas.example".into()),
                ..Settings::default()
            }
            .validate()
            .is_err()
        );
    }
}
