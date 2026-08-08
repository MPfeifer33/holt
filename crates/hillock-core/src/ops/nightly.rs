//! Nightly maintenance pipeline.
//!
//! Runs maintenance: decay and prune.
//!
//! Run state is persisted in SQLite so missed runs are caught on restart.

use chrono::{DateTime, Utc};

use crate::index::VectorIndex;
use crate::model::*;
use crate::ops;
use crate::store::MemoryStore;

/// Run the nightly pipeline: snapshot, then maintenance.
pub async fn run(
    store: &dyn MemoryStore,
    index: &dyn VectorIndex,
    agent_id: &str,
    config: NightlyConfig,
) -> Result<NightlyOutput, anyhow::Error> {
    let start = std::time::Instant::now();
    let ran_at = Utc::now();

    // One flag governs the whole pipeline, read from the config of the stage that does the
    // deleting. See `MaintenanceConfig::dry_run` for why it lives there and not here.
    let dry_run = config.maintenance.dry_run;

    // ── Stage 0: Snapshot ──
    //
    // Everything below this point deletes rows. A configured
    // backup that fails is a hard stop: running the pipeline anyway would be doing the
    // destructive work with the safety net explicitly requested and absent.
    // A dry run has nothing to protect, so it does not spend a full copy of the database
    // rehearsing.
    if let Some(backup) = config.backup.as_ref().filter(|_| !dry_run) {
        let dest = unique_snapshot_path(&backup.dir, ran_at);
        store.backup_to(&dest).await.map_err(|e| {
            anyhow::anyhow!(
                "nightly aborted: pre-run backup to {} failed: {e}",
                dest.display()
            )
        })?;
        tracing::info!(path = %dest.display(), "nightly pre-run snapshot written");

        match prune_snapshots(&backup.dir, backup.keep_days, ran_at) {
            Ok(0) => {}
            Ok(n) => tracing::info!(removed = n, "pruned expired snapshots"),
            // Retention is housekeeping; a failure here must not block maintenance now
            // that the snapshot itself is safely on disk.
            Err(e) => tracing::warn!("snapshot retention sweep failed: {e}"),
        }
    }

    // ── Stage 1: Maintenance ──
    let maintenance_result =
        ops::maintenance::run(store, index, agent_id, config.maintenance).await;
    let (m_decayed, m_pruned, m_tombstones) = match &maintenance_result {
        Ok(o) => (o.memories_decayed, o.memories_pruned, o.tombstones_cleaned),
        Err(e) => {
            tracing::error!("nightly maintenance failed: {e}");
            (0, 0, 0)
        }
    };

    let total_duration_ms = start.elapsed().as_millis() as u64;

    let output = NightlyOutput {
        memories_decayed: m_decayed,
        memories_pruned: m_pruned,
        tombstones_cleaned: m_tombstones,
        total_duration_ms,
        ran_at,
    };

    // Record the run in SQLite.
    //
    // Not on a dry run: `is_overdue` measures from the last recorded run, so a rehearsal
    // would mark maintenance as done and suppress the next real one.
    if !dry_run {
        let summary = format!("decayed:{m_decayed} pruned:{m_pruned}");
        let record = NightlyRunRecord {
            ran_at,
            agent_id: agent_id.to_string(),
            duration_ms: total_duration_ms,
            summary,
        };
        if let Err(e) = store.record_nightly_run(&record).await {
            tracing::error!("failed to record nightly run: {e}");
        }
    }

    Ok(output)
}

/// Check if a nightly run is overdue and should be triggered.
///
/// Returns true if the last run was more than `max_gap_hours` ago,
/// or if no run has ever been recorded.
pub async fn is_overdue(
    store: &dyn MemoryStore,
    agent_id: &str,
    max_gap_hours: u32,
) -> Result<bool, anyhow::Error> {
    let last = store.last_nightly_run(agent_id).await?;
    match last {
        None => Ok(true), // never run
        Some(record) => {
            let elapsed = Utc::now() - record.ran_at;
            Ok(elapsed.num_hours() >= max_gap_hours as i64)
        }
    }
}

// ── Snapshots ───────────────────────────────────────────────────────

/// Prefix shared by every snapshot this pipeline writes.
const SNAPSHOT_PREFIX: &str = "hillock-";
/// `YYYYMMDD-HHMMSS`
const SNAPSHOT_STAMP: &str = "%Y%m%d-%H%M%S";

/// Snapshot filename for a run.
///
/// Timestamped to the second, deliberately. The `backup-hillock.sh` this supersedes named
/// by date alone, so a second run in the same day silently overwrote that morning's
/// snapshot — replacing a pre-maintenance copy with a post-maintenance one.
fn snapshot_filename(ran_at: DateTime<Utc>) -> String {
    format!("{SNAPSHOT_PREFIX}{}.db", ran_at.format(SNAPSHOT_STAMP))
}

/// Parse a snapshot's timestamp, or `None` if the name is not one of ours.
///
/// Deliberately strict: retention deletes files, so anything it cannot positively
/// identify as a snapshot it wrote is left alone. That includes date-only names from the
/// old shell script and any manually-kept copy.
fn snapshot_timestamp(file_name: &str) -> Option<DateTime<Utc>> {
    let stem = file_name
        .strip_prefix(SNAPSHOT_PREFIX)?
        .strip_suffix(".db")?;
    let parse = |s: &str| {
        chrono::NaiveDateTime::parse_from_str(s, SNAPSHOT_STAMP)
            .ok()
            .map(|naive| naive.and_utc())
    };

    parse(stem).or_else(|| {
        // Collision suffix, e.g. `hillock-20260726-012011-2.db`.
        let (base, suffix) = stem.rsplit_once('-')?;
        if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        parse(base)
    })
}

/// First free snapshot path for a run.
///
/// Two runs inside the same second would otherwise collide, and `backup_to` refuses to
/// overwrite — which, combined with abort-on-backup-failure, would block maintenance over
/// a filename rather than a real problem. Rare on a daily schedule, immediate on a manual
/// rerun.
fn unique_snapshot_path(dir: &std::path::Path, ran_at: DateTime<Utc>) -> std::path::PathBuf {
    let base = dir.join(snapshot_filename(ran_at));
    if !base.exists() {
        return base;
    }
    let stamp = ran_at.format(SNAPSHOT_STAMP);
    (2..)
        .map(|n| dir.join(format!("{SNAPSHOT_PREFIX}{stamp}-{n}.db")))
        .find(|p| !p.exists())
        .expect("an unused snapshot suffix always exists")
}

/// Remove snapshots older than `keep_days`. Returns how many were removed.
fn prune_snapshots(
    dir: &std::path::Path,
    keep_days: u32,
    now: DateTime<Utc>,
) -> Result<usize, std::io::Error> {
    let cutoff = now - chrono::Duration::days(keep_days as i64);
    let mut removed = 0;

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(stamp) = snapshot_timestamp(name) else {
            continue;
        };
        if stamp < cutoff {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(e) => tracing::warn!(file = %name, "failed to remove expired snapshot: {e}"),
            }
        }
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
            .unwrap()
            .and_utc()
    }

    #[test]
    fn snapshot_names_are_unique_within_a_day() {
        let morning = snapshot_filename(ts("2026-07-25 03:01:00"));
        let evening = snapshot_filename(ts("2026-07-25 21:14:00"));
        assert_ne!(
            morning, evening,
            "two runs in one day must not share a filename"
        );
        assert_eq!(morning, "hillock-20260725-030100.db");
    }

    #[test]
    fn snapshot_timestamp_roundtrips() {
        let when = ts("2026-07-25 21:14:09");
        assert_eq!(snapshot_timestamp(&snapshot_filename(when)), Some(when));
    }

    /// Retention deletes files, so it must recognise only its own.
    #[test]
    fn retention_ignores_files_it_did_not_write() {
        for name in [
            "hillock-20260725.db",            // old shell-script format (date only)
            "pre-remediation-20260725.db",    // manual milestone backup
            "hillock.db",                     // the live database
            "hillock-20260725-030100.db.bak", // not a .db
            "notes.md",
        ] {
            assert_eq!(snapshot_timestamp(name), None, "would have deleted {name}");
        }
    }

    #[test]
    fn retention_removes_only_expired_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let now = ts("2026-07-25 03:00:00");

        let fresh = snapshot_filename(ts("2026-07-24 03:00:00"));
        let expired = snapshot_filename(ts("2026-07-01 03:00:00"));
        let foreign = "pre-remediation-20260725.db";
        for name in [fresh.as_str(), expired.as_str(), foreign] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }

        let removed = prune_snapshots(dir.path(), 7, now).unwrap();

        assert_eq!(removed, 1);
        assert!(dir.path().join(&fresh).exists(), "fresh snapshot removed");
        assert!(!dir.path().join(&expired).exists(), "expired snapshot kept");
        assert!(dir.path().join(foreign).exists(), "foreign file removed");
    }
}

#[cfg(test)]
mod collision_tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
            .unwrap()
            .and_utc()
    }

    /// Two runs in the same second must not collide. `backup_to` refuses to overwrite, and
    /// a failed backup aborts the pipeline -- so a collision would block maintenance over
    /// a filename.
    #[test]
    fn snapshot_path_steps_aside_on_collision() {
        let dir = tempfile::tempdir().unwrap();
        let when = ts("2026-07-26 01:20:11");

        let first = unique_snapshot_path(dir.path(), when);
        assert!(first.ends_with("hillock-20260726-012011.db"));
        std::fs::write(&first, b"x").unwrap();

        let second = unique_snapshot_path(dir.path(), when);
        assert_ne!(second, first);
        assert!(second.ends_with("hillock-20260726-012011-2.db"));
        std::fs::write(&second, b"x").unwrap();

        let third = unique_snapshot_path(dir.path(), when);
        assert!(third.ends_with("hillock-20260726-012011-3.db"));
    }

    /// Retention still has to recognise collision-suffixed snapshots, or they accumulate
    /// forever.
    #[test]
    fn collision_suffixed_snapshots_are_still_reapable() {
        let when = ts("2026-07-26 01:20:11");
        assert_eq!(
            snapshot_timestamp("hillock-20260726-012011-2.db"),
            Some(when)
        );
        assert_eq!(
            snapshot_timestamp("hillock-20260726-012011-17.db"),
            Some(when)
        );
        // Still strict about everything else.
        assert_eq!(snapshot_timestamp("hillock-20260726-012011-x.db"), None);
        assert_eq!(snapshot_timestamp("hillock-20260726-012011-.db"), None);
        assert_eq!(snapshot_timestamp("hillock-20260725.db"), None);
    }

    #[test]
    fn retention_reaps_collision_suffixed_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let now = ts("2026-07-26 03:00:00");
        for name in ["hillock-20260701-030000.db", "hillock-20260701-030000-2.db"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }

        assert_eq!(prune_snapshots(dir.path(), 7, now).unwrap(), 2);
    }
}
