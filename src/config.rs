use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Default)]
struct RawConfig {
    trakt_client_id: Option<String>,
    trakt_client_secret: Option<String>,
    tmdb_api_key: Option<String>,
    data_dir: Option<String>,
}

/// Runtime configuration for the tool.
///
/// Config file location (highest to lowest precedence):
///   1. `--config <path>` CLI flag
///   2. `TRAKT_CONFIG_FILE` env var
///   3. `~/.config/trakt-letterboxd/config.toml`
///
/// Individual env var overrides (take precedence over file values):
///   `TRAKT_CLIENT_ID`, `TRAKT_CLIENT_SECRET`, `TMDB_API_KEY`, `DATA_DIR`
#[allow(dead_code)]
#[derive(Debug)]
pub struct Config {
    pub trakt_client_id: String,
    pub trakt_client_secret: String,
    pub tmdb_api_key: Option<String>,
    pub data_dir: PathBuf,
}

impl Config {
    pub fn load(config_file: Option<&Path>) -> Result<Self, String> {
        let env: HashMap<String, String> = std::env::vars().collect();
        Self::from_parts(config_file, &env)
    }

    /// Inner loader — accepts an explicit env map so tests can isolate from the real environment.
    pub(crate) fn from_parts(
        config_file: Option<&Path>,
        env: &HashMap<String, String>,
    ) -> Result<Self, String> {
        let config_path = config_file
            .map(|p| p.to_path_buf())
            .or_else(|| env.get("TRAKT_CONFIG_FILE").map(PathBuf::from))
            .or_else(|| {
                env.get("HOME").map(|h| {
                    PathBuf::from(h)
                        .join(".config")
                        .join("trakt-letterboxd")
                        .join("config.toml")
                })
            });

        let mut raw: RawConfig = match config_path {
            Some(ref path) if path.exists() => {
                let content = std::fs::read_to_string(path)
                    .map_err(|e| format!("failed to read config file {}: {}", path.display(), e))?;
                toml::from_str(&content)
                    .map_err(|e| format!("invalid config file {}: {}", path.display(), e))?
            }
            _ => RawConfig::default(),
        };

        if let Some(v) = env.get("TRAKT_CLIENT_ID") {
            raw.trakt_client_id = Some(v.clone());
        }
        if let Some(v) = env.get("TRAKT_CLIENT_SECRET") {
            raw.trakt_client_secret = Some(v.clone());
        }
        if let Some(v) = env.get("TMDB_API_KEY") {
            raw.tmdb_api_key = Some(v.clone());
        }
        if let Some(v) = env.get("DATA_DIR") {
            raw.data_dir = Some(v.clone());
        }

        let trakt_client_id = raw.trakt_client_id.ok_or_else(|| {
            "missing required config: trakt_client_id\n  \
             → set TRAKT_CLIENT_ID env var or add `trakt_client_id = \"...\"` to config file"
                .to_string()
        })?;

        let trakt_client_secret = raw.trakt_client_secret.ok_or_else(|| {
            "missing required config: trakt_client_secret\n  \
             → set TRAKT_CLIENT_SECRET env var or add `trakt_client_secret = \"...\"` to config file"
                .to_string()
        })?;

        let data_dir = raw
            .data_dir
            .map(PathBuf::from)
            .or_else(|| {
                env.get("HOME").map(|h| {
                    PathBuf::from(h)
                        .join(".local")
                        .join("share")
                        .join("trakt-letterboxd")
                })
            })
            .unwrap_or_else(|| PathBuf::from("data"));

        Ok(Config {
            trakt_client_id,
            trakt_client_secret,
            tmdb_api_key: raw.tmdb_api_key,
            data_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn write_toml(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "{content}").unwrap();
        f
    }

    #[test]
    fn valid_config_parses() {
        let f = write_toml(
            "trakt_client_id = \"abc123\"\n\
             trakt_client_secret = \"secret456\"\n\
             tmdb_api_key = \"tmdb789\"\n\
             data_dir = \"/tmp/sync-data\"\n",
        );
        let cfg = Config::from_parts(Some(f.path()), &env(&[])).unwrap();
        assert_eq!(cfg.trakt_client_id, "abc123");
        assert_eq!(cfg.trakt_client_secret, "secret456");
        assert_eq!(cfg.tmdb_api_key, Some("tmdb789".to_string()));
        assert_eq!(cfg.data_dir, PathBuf::from("/tmp/sync-data"));
    }

    #[test]
    fn optional_fields_use_defaults() {
        let f = write_toml(
            "trakt_client_id = \"abc\"\n\
             trakt_client_secret = \"xyz\"\n",
        );
        let cfg = Config::from_parts(Some(f.path()), &env(&[("HOME", "/home/user")])).unwrap();
        assert_eq!(cfg.tmdb_api_key, None);
        assert_eq!(
            cfg.data_dir,
            PathBuf::from("/home/user/.local/share/trakt-letterboxd")
        );
    }

    #[test]
    fn missing_client_id_yields_clear_error() {
        let f = write_toml("trakt_client_secret = \"xyz\"\n");
        let err = Config::from_parts(Some(f.path()), &env(&[])).unwrap_err();
        assert!(
            err.contains("trakt_client_id"),
            "expected error to name the missing field, got: {err}"
        );
    }

    #[test]
    fn missing_client_secret_yields_clear_error() {
        let f = write_toml("trakt_client_id = \"abc\"\n");
        let err = Config::from_parts(Some(f.path()), &env(&[])).unwrap_err();
        assert!(
            err.contains("trakt_client_secret"),
            "expected error to name the missing field, got: {err}"
        );
    }

    #[test]
    fn env_vars_override_file_values() {
        let f = write_toml(
            "trakt_client_id = \"file_id\"\n\
             trakt_client_secret = \"file_secret\"\n",
        );
        let cfg = Config::from_parts(
            Some(f.path()),
            &env(&[
                ("TRAKT_CLIENT_ID", "env_id"),
                ("TRAKT_CLIENT_SECRET", "env_secret"),
            ]),
        )
        .unwrap();
        assert_eq!(cfg.trakt_client_id, "env_id");
        assert_eq!(cfg.trakt_client_secret, "env_secret");
    }

    #[test]
    fn env_vars_alone_are_sufficient() {
        let cfg = Config::from_parts(
            None,
            &env(&[
                ("TRAKT_CLIENT_ID", "env_id"),
                ("TRAKT_CLIENT_SECRET", "env_secret"),
            ]),
        )
        .unwrap();
        assert_eq!(cfg.trakt_client_id, "env_id");
        assert_eq!(cfg.trakt_client_secret, "env_secret");
    }
}
