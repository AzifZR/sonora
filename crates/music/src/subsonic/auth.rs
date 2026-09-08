use std::path::PathBuf;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::credentials;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Credentials {
    pub server: String,
    pub username: String,
    pub password: String,
}

fn path() -> PathBuf {
    credentials::dir("subsonic").join(credentials::FILE)
}

pub fn normalize_server(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        anyhow::bail!("the server address is empty");
    }
    let with_scheme = match trimmed.contains("://") {
        true => trimmed.to_owned(),
        false => format!("http://{trimmed}"),
    };
    Ok(with_scheme)
}

pub fn load() -> Option<Credentials> {
    let bytes = std::fs::read(path()).ok()?;
    let mut credentials: Credentials = serde_json::from_slice(&bytes).ok()?;
    credentials.server = normalize_server(&credentials.server).ok()?;
    match credentials.username.is_empty() {
        true => None,
        false => Some(credentials),
    }
}

pub fn store(stored: &Credentials) -> Result<()> {
    let bytes =
        serde_json::to_vec_pretty(stored).context("cannot serialize subsonic credentials")?;
    credentials::write(&path(), &bytes).context("cannot store subsonic credentials")
}

pub fn forget() {
    credentials::remove(&path());
}
