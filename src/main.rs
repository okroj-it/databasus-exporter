//! Prometheus exporter for Databasus backups.
//!
//! Reads the Databasus REST API as a (preferably view-only) user and serves
//! the result on /metrics. Collection happens on scrape, at most once per
//! CACHE_SECONDS.

mod api;
mod metrics;

use std::env;
use std::process::ExitCode;
use std::str::FromStr;
use std::time::{Duration, Instant};

use metrics::{BackupInfo, DatabaseInfo, Snapshot};

/// How far back to look for the latest completed backup.
const BACKUP_LOOKBACK: u32 = 20;

struct Config {
    url: String,
    email: String,
    password: String,
    listen: String,
    cache: Duration,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let required = |k: &str| env::var(k).map_err(|_| format!("{k} is not set"));
        let cache = env::var("CACHE_SECONDS")
            .ok()
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|_| format!("CACHE_SECONDS: not a number: {v}"))
            })
            .transpose()?
            .unwrap_or(30);
        Ok(Config {
            url: env::var("DATABASUS_URL").unwrap_or_else(|_| "http://localhost:4005".into()),
            email: required("DATABASUS_EMAIL")?,
            password: required("DATABASUS_PASSWORD")?,
            listen: env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:9810".into()),
            cache: Duration::from_secs(cache),
        })
    }
}

fn unix_seconds(rfc3339: &str) -> Option<f64> {
    let ts = jiff::Timestamp::from_str(rfc3339).ok()?;
    Some(ts.as_millisecond() as f64 / 1000.0)
}

fn backup_info(b: &api::Backup) -> Option<BackupInfo> {
    const MIB: f64 = 1024.0 * 1024.0;
    Some(BackupInfo {
        status: b.status.clone(),
        verification: b.restore_verification_status.clone(),
        created: unix_seconds(&b.created_at)?,
        size_bytes: b.backup_size_mb.map(|m| m * MIB),
        raw_size_bytes: b.backup_raw_db_size_mb.map(|m| m * MIB),
        duration_seconds: b.backup_duration_ms.map(|ms| ms / 1000.0),
    })
}

/// One pass over the API. Per-database failures are logged and leave that
/// database's backup fields empty; they also mark the snapshot as not up.
fn collect(client: &mut api::Client) -> Snapshot {
    let started = Instant::now();
    let mut snap = Snapshot {
        up: true,
        ..Default::default()
    };

    let workspaces = match client.workspaces() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("listing workspaces: {e}");
            snap.up = false;
            Vec::new()
        }
    };

    for ws in &workspaces {
        let databases = match client.databases(&ws.id) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("listing databases of {}: {e}", ws.name);
                snap.up = false;
                continue;
            }
        };
        for db in databases {
            let mut info = DatabaseInfo {
                workspace: ws.name.clone(),
                name: db.name.clone(),
                kind: db.kind.clone(),
                healthy: db.health_status.as_deref() == Some("AVAILABLE"),
                ..Default::default()
            };
            match client.backups(&db.id, BACKUP_LOOKBACK) {
                Ok(page) => {
                    info.backups_total = Some(page.total);
                    info.latest = page.backups.first().and_then(backup_info);
                    info.last_success = page
                        .backups
                        .iter()
                        .find(|b| b.status == "COMPLETED")
                        .and_then(backup_info);
                }
                Err(e) => {
                    eprintln!("backups of {}: {e}", db.name);
                    snap.up = false;
                }
            }
            match client.backup_config(&db.id) {
                Ok(c) => info.backups_enabled = Some(c.is_backups_enabled),
                Err(e) => {
                    eprintln!("backup config of {}: {e}", db.name);
                    snap.up = false;
                }
            }
            snap.databases.push(info);
        }
    }

    snap.duration_seconds = started.elapsed().as_secs_f64();
    snap
}

fn respond(req: tiny_http::Request, status: u16, content_type: &str, body: String) {
    let header = tiny_http::Header::from_bytes("Content-Type", content_type).expect("valid header");
    let resp = tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(header);
    if let Err(e) = req.respond(resp) {
        eprintln!("writing response: {e}");
    }
}

fn main() -> ExitCode {
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config: {e}");
            return ExitCode::FAILURE;
        }
    };
    let server = match tiny_http::Server::http(&cfg.listen) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("listening on {}: {e}", cfg.listen);
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "databasus-exporter listening on {}, reading {}",
        cfg.listen, cfg.url
    );

    let mut client = api::Client::new(&cfg.url, cfg.email, cfg.password);
    let mut cached: Option<(Instant, String)> = None;

    // Requests are handled one at a time: scrapes are rare and a collection
    // takes well under a second, so there is nothing to gain from threads.
    for req in server.incoming_requests() {
        match req.url() {
            "/metrics" => {
                let fresh = matches!(&cached, Some((at, _)) if at.elapsed() < cfg.cache);
                if !fresh {
                    let body = metrics::render(&collect(&mut client));
                    cached = Some((Instant::now(), body));
                }
                let body = cached.as_ref().map(|(_, b)| b.clone()).unwrap_or_default();
                respond(req, 200, "text/plain; version=0.0.4", body);
            }
            "/healthz" => respond(req, 200, "text/plain", "ok\n".into()),
            _ => respond(req, 404, "text/plain", "not found\n".into()),
        }
    }
    ExitCode::SUCCESS
}
