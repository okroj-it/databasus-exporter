//! Minimal client for the parts of the Databasus REST API the exporter reads.
//!
//! Databasus has no API keys, so the client signs in with a user's email and
//! password and keeps the returned token. When a request comes back 401 (the
//! token expired, or the password was changed) it signs in again once and
//! retries.

use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub enum Error {
    Http(ureq::Error),
    Json(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Http(e) => write!(f, "{e}"),
            Error::Json(e) => write!(f, "unexpected response: {e}"),
        }
    }
}

impl From<ureq::Error> for Error {
    fn from(e: ureq::Error) -> Self {
        Error::Http(e)
    }
}

#[derive(Debug, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Database {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub health_status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub status: String,
    pub restore_verification_status: Option<String>,
    pub backup_size_mb: Option<f64>,
    pub backup_raw_db_size_mb: Option<f64>,
    pub backup_duration_ms: Option<f64>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct BackupPage {
    pub backups: Vec<Backup>,
    pub total: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupConfig {
    pub is_backups_enabled: bool,
}

#[derive(Deserialize)]
struct WorkspaceList {
    workspaces: Vec<Workspace>,
}

#[derive(Deserialize)]
struct SignIn {
    token: String,
}

pub struct Client {
    base: String,
    email: String,
    password: String,
    agent: ureq::Agent,
    token: Option<String>,
}

impl Client {
    /// `base` is the Databasus URL without the /api/v1 suffix.
    pub fn new(base: &str, email: String, password: String) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .into();
        Client {
            base: format!("{}/api/v1", base.trim_end_matches('/')),
            email,
            password,
            agent,
            token: None,
        }
    }

    fn sign_in(&mut self) -> Result<(), Error> {
        let body = serde_json::json!({ "email": self.email, "password": self.password });
        let mut resp = self
            .agent
            .post(format!("{}/users/signin", self.base))
            .send_json(&body)?;
        let s: SignIn = resp
            .body_mut()
            .read_json()
            .map_err(|e| Error::Json(e.to_string()))?;
        self.token = Some(s.token);
        Ok(())
    }

    fn get<T: DeserializeOwned>(&mut self, path: &str) -> Result<T, Error> {
        if self.token.is_none() {
            self.sign_in()?;
        }
        match self.try_get(path) {
            Err(Error::Http(ureq::Error::StatusCode(401))) => {
                self.sign_in()?;
                self.try_get(path)
            }
            other => other,
        }
    }

    fn try_get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        let token = self.token.as_deref().unwrap_or_default();
        let mut resp = self
            .agent
            .get(format!("{}{path}", self.base))
            .header("Authorization", format!("Bearer {token}"))
            .call()?;
        resp.body_mut()
            .read_json()
            .map_err(|e| Error::Json(format!("{path}: {e}")))
    }

    pub fn workspaces(&mut self) -> Result<Vec<Workspace>, Error> {
        Ok(self.get::<WorkspaceList>("/workspaces")?.workspaces)
    }

    pub fn databases(&mut self, workspace_id: &str) -> Result<Vec<Database>, Error> {
        self.get(&format!("/databases?workspace_id={workspace_id}"))
    }

    /// Newest first.
    pub fn backups(&mut self, database_id: &str, limit: u32) -> Result<BackupPage, Error> {
        self.get(&format!("/backups?database_id={database_id}&limit={limit}"))
    }

    pub fn backup_config(&mut self, database_id: &str) -> Result<BackupConfig, Error> {
        self.get(&format!("/backup-configs/database/{database_id}"))
    }
}
