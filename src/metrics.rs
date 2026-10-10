//! Prometheus text exposition for one collected snapshot.

use std::fmt::Write;

/// Every status Databasus can report, so the one-hot gauges always carry
/// all of them (0 or 1) and alerts never depend on a series appearing.
pub const BACKUP_STATUSES: [&str; 4] = ["IN_PROGRESS", "COMPLETED", "FAILED", "CANCELED"];
pub const VERIFICATION_STATUSES: [&str; 3] =
    ["NOT_VERIFIED", "VERIFIED_SUCCESSFUL", "VERIFICATION_FAILED"];

#[derive(Debug, Clone, Default)]
pub struct BackupInfo {
    pub status: String,
    pub verification: Option<String>,
    /// Unix seconds.
    pub created: f64,
    pub size_bytes: Option<f64>,
    pub raw_size_bytes: Option<f64>,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct DatabaseInfo {
    pub workspace: String,
    pub name: String,
    pub kind: String,
    pub healthy: bool,
    pub backups_enabled: Option<bool>,
    pub backups_total: Option<u64>,
    pub latest: Option<BackupInfo>,
    pub last_success: Option<BackupInfo>,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub up: bool,
    pub duration_seconds: f64,
    pub databases: Vec<DatabaseInfo>,
}

fn escape(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

struct Out(String);

impl Out {
    fn header(&mut self, name: &str, help: &str) {
        let _ = writeln!(self.0, "# HELP {name} {help}");
        let _ = writeln!(self.0, "# TYPE {name} gauge");
    }

    fn sample(&mut self, name: &str, labels: &[(&str, &str)], value: f64) {
        self.0.push_str(name);
        if !labels.is_empty() {
            self.0.push('{');
            for (i, (k, v)) in labels.iter().enumerate() {
                if i > 0 {
                    self.0.push(',');
                }
                let _ = write!(self.0, "{k}=\"{}\"", escape(v));
            }
            self.0.push('}');
        }
        let _ = writeln!(self.0, " {value}");
    }

    /// One gauge per database, skipping databases where `f` has no value.
    fn per_db(
        &mut self,
        dbs: &[DatabaseInfo],
        name: &str,
        help: &str,
        f: impl Fn(&DatabaseInfo) -> Option<f64>,
    ) {
        self.header(name, help);
        for db in dbs {
            if let Some(v) = f(db) {
                self.sample(
                    name,
                    &[("workspace", &db.workspace), ("database", &db.name)],
                    v,
                );
            }
        }
    }

    fn one_hot(
        &mut self,
        dbs: &[DatabaseInfo],
        name: &str,
        help: &str,
        values: &[&str],
        f: impl Fn(&DatabaseInfo) -> Option<&str>,
    ) {
        self.header(name, help);
        for db in dbs {
            let Some(current) = f(db) else { continue };
            for v in values {
                let hit = if *v == current { 1.0 } else { 0.0 };
                self.sample(
                    name,
                    &[
                        ("workspace", &db.workspace),
                        ("database", &db.name),
                        ("status", v),
                    ],
                    hit,
                );
            }
        }
    }
}

fn flag(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

pub fn render(s: &Snapshot) -> String {
    let mut o = Out(String::new());
    let dbs = &s.databases;

    o.header(
        "databasus_up",
        "Whether the last collection from the Databasus API succeeded.",
    );
    o.sample("databasus_up", &[], flag(s.up));
    o.header(
        "databasus_scrape_duration_seconds",
        "Time the last collection from the Databasus API took.",
    );
    o.sample("databasus_scrape_duration_seconds", &[], s.duration_seconds);

    o.header("databasus_database_info", "Databases known to Databasus.");
    for db in dbs {
        o.sample(
            "databasus_database_info",
            &[
                ("workspace", &db.workspace),
                ("database", &db.name),
                ("type", &db.kind),
            ],
            1.0,
        );
    }

    o.per_db(
        dbs,
        "databasus_database_healthy",
        "Whether Databasus can reach the database.",
        |d| Some(flag(d.healthy)),
    );
    o.per_db(
        dbs,
        "databasus_backups_enabled",
        "Whether scheduled backups are enabled.",
        |d| d.backups_enabled.map(flag),
    );
    o.per_db(
        dbs,
        "databasus_backups",
        "Number of backups Databasus keeps for the database.",
        |d| d.backups_total.map(|n| n as f64),
    );

    o.per_db(
        dbs,
        "databasus_backup_last_timestamp_seconds",
        "Start time of the most recent backup, whatever its status.",
        |d| d.latest.as_ref().map(|b| b.created),
    );
    o.one_hot(
        dbs,
        "databasus_backup_last_status",
        "Status of the most recent backup (1 for the current status).",
        &BACKUP_STATUSES,
        |d| d.latest.as_ref().map(|b| b.status.as_str()),
    );

    o.per_db(
        dbs,
        "databasus_backup_last_success_timestamp_seconds",
        "Start time of the most recent completed backup.",
        |d| d.last_success.as_ref().map(|b| b.created),
    );
    o.per_db(
        dbs,
        "databasus_backup_last_success_size_bytes",
        "Size of the most recent completed backup file.",
        |d| d.last_success.as_ref().and_then(|b| b.size_bytes),
    );
    o.per_db(
        dbs,
        "databasus_backup_last_success_raw_size_bytes",
        "Database size covered by the most recent completed backup.",
        |d| d.last_success.as_ref().and_then(|b| b.raw_size_bytes),
    );
    o.per_db(
        dbs,
        "databasus_backup_last_success_duration_seconds",
        "How long the most recent completed backup took.",
        |d| d.last_success.as_ref().and_then(|b| b.duration_seconds),
    );
    o.one_hot(
        dbs,
        "databasus_backup_last_verification",
        "Restore verification of the most recent completed backup (1 for the current status).",
        &VERIFICATION_STATUSES,
        |d| {
            d.last_success
                .as_ref()
                .and_then(|b| b.verification.as_deref())
        },
    );

    o.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        let ok = BackupInfo {
            status: "COMPLETED".into(),
            verification: Some("VERIFIED_SUCCESSFUL".into()),
            created: 1_760_000_000.0,
            size_bytes: Some(1024.0),
            raw_size_bytes: Some(4096.0),
            duration_seconds: Some(4.8),
        };
        Snapshot {
            up: true,
            duration_seconds: 0.25,
            databases: vec![
                DatabaseInfo {
                    workspace: "Homelab".into(),
                    name: "grafana".into(),
                    kind: "POSTGRES_LOGICAL".into(),
                    healthy: true,
                    backups_enabled: Some(true),
                    backups_total: Some(7),
                    latest: Some(BackupInfo {
                        status: "FAILED".into(),
                        created: 1_760_086_400.0,
                        ..Default::default()
                    }),
                    last_success: Some(ok),
                },
                DatabaseInfo {
                    workspace: "Homelab".into(),
                    name: "new \"db\"".into(),
                    kind: "POSTGRES_LOGICAL".into(),
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn renders_core_samples() {
        let out = render(&sample());
        assert!(out.contains("databasus_up 1\n"));
        assert!(out.contains(
            "databasus_backup_last_success_timestamp_seconds{workspace=\"Homelab\",database=\"grafana\"} 1760000000\n"
        ));
        assert!(out.contains(
            "databasus_backup_last_success_duration_seconds{workspace=\"Homelab\",database=\"grafana\"} 4.8\n"
        ));
        assert!(out.contains("databasus_backups{workspace=\"Homelab\",database=\"grafana\"} 7\n"));
    }

    #[test]
    fn status_is_one_hot_over_all_values() {
        let out = render(&sample());
        let lines: Vec<_> = out
            .lines()
            .filter(|l| l.starts_with("databasus_backup_last_status{") && l.contains("\"grafana\""))
            .collect();
        assert_eq!(lines.len(), BACKUP_STATUSES.len());
        assert!(
            lines
                .iter()
                .any(|l| l.contains("status=\"FAILED\"") && l.ends_with(" 1"))
        );
        assert_eq!(lines.iter().filter(|l| l.ends_with(" 1")).count(), 1);
    }

    #[test]
    fn databases_without_backups_have_no_backup_samples() {
        let out = render(&sample());
        let new_db = out
            .lines()
            .filter(|l| l.contains("new \\\"db\\\""))
            .collect::<Vec<_>>();
        // info and healthy only; nothing about backups it never had
        assert_eq!(new_db.len(), 2, "{new_db:?}");
    }

    #[test]
    fn escapes_label_values() {
        assert_eq!(escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn every_metric_has_help_and_type() {
        let out = render(&sample());
        for line in out.lines().filter(|l| !l.starts_with('#')) {
            let name = line.split(['{', ' ']).next().unwrap();
            assert!(
                out.contains(&format!("# TYPE {name} gauge")),
                "missing TYPE for {name}"
            );
        }
    }
}
