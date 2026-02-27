use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub credentials: Credentials,
    pub general: General,
}

#[derive(Debug, Deserialize)]
pub struct Credentials {
    pub private_key: String,
    pub proxy_wallet: String,
}

#[derive(Debug, Deserialize)]
pub struct General {
    pub log_level: String,
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let contents = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }
}
