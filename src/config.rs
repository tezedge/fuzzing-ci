use std::{collections::HashMap, ffi::OsStr, fs::File, io::Read, path::PathBuf};

use derive_new::new;
use failure::{Error, ResultExt};
use serde::Deserialize;
use url::Url;

#[derive(Clone, Deserialize, new)]
pub struct Config {
    pub address: String,
    pub url: Option<Url>,
    pub branches: Vec<String>,
    pub corpus: Option<String>,
    pub kcov: Option<KCov>,
    pub honggfuzz: HashMap<String, Honggfuzz>,
    #[serde(default)]
    pub feedback: Feedback,
    pub slack: Option<Slack>,
    pub reports_path: PathBuf,
}

#[derive(Clone, Deserialize, new)]
pub struct KCov {
    pub kcov_args: Vec<String>,
}

#[derive(Clone, Deserialize, new)]
pub struct Feedback {
    #[serde(default = "Feedback::default_update_timeout")]
    pub update_timeout: u64,
    #[serde(default = "Feedback::default_no_update_timeout")]
    pub no_update_timeout: u64,
}

impl Feedback {
    fn default_update_timeout() -> u64 {
        10 * 60
    }
    fn default_no_update_timeout() -> u64 {
        24 * 60 * 60
    }
}

impl Default for Feedback {
    fn default() -> Self {
        Self {
            update_timeout: Self::default_update_timeout(),
            no_update_timeout: Self::default_no_update_timeout(),
        }
    }
}

#[derive(Clone, Deserialize, new)]
pub struct Honggfuzz {
    pub path: Option<String>,
    pub targets: Vec<String>,
}

#[derive(Clone, Deserialize, new)]
pub struct Slack {
    pub channel: String,
    #[serde(default = "Slack::get_token")]
    pub token: String,
}

impl Config {
    pub fn read(file: impl AsRef<OsStr>) -> Result<Self, Error> {
        let mut config = String::new();
        File::open(file.as_ref()).and_then(|mut f| f.read_to_string(&mut config))?;
        let mut config: Config = toml::from_str(&config)?;

        if let Some(ref mut corpus) = config.corpus {
            let path = PathBuf::from(&corpus);
            if path.is_relative() {
                *corpus = PathBuf::from(file.as_ref())
                    .canonicalize()
                    .with_context(|e| {
                        format!(
                            "cannot canonicalize path {}: {}",
                            file.as_ref().to_string_lossy(),
                            e
                        )
                    })?
                    .parent()
                    .unwrap()
                    .join(path)
                    .to_string_lossy()
                    .into_owned();
            }
        }

            let path = PathBuf::from(&config.reports_path);
            if path.is_relative() {
                config.reports_path = PathBuf::from(file.as_ref())
                    .canonicalize()
                    .with_context(|e| {
                        format!(
                            "cannot canonicalize path {}: {}",
                            file.as_ref().to_string_lossy(),
                            e
                        )
                    })?
                    .parent()
                    .unwrap()
                    .join(path);
            }

        Ok(config)
    }
}

impl Slack {
    fn get_token() -> String {
        std::env::var("SLACK_AUTH_TOKEN").unwrap_or(String::new())
    }
}
