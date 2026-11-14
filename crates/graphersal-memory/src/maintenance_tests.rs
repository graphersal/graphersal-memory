//! Store maintenance on real stores in temporary data directories: the automatic checkpoint at
//! a small threshold, the prune keeping the last N days (the clock injected as the `now` of
//! `prune_old_history`, against the engine's commit times), the copy lock, and a bench fork
//! that still opens after a prune.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::maintenance::{self, Settings};
use crate::mcp::{self, StoreRuntime};
use crate::mcp_tests::TempDir;

fn open(data: &TempDir, settings: &Settings) -> StoreRuntime {
    StoreRuntime::open_with(
        crate::registry::DEFAULT_STORE,
        &crate::store::default_store_dir(data.path()),
        Arc::new(mcp::endpoint()),
        settings,
    )
    .unwrap()
}

/// Writes `count` memories of about 1 KiB each.
fn write(runtime: &StoreRuntime, from: usize, count: usize) {
    for i in from..from + count {
        runtime
            .call(
                "remember",
                json!({"slug": format!("note-{i}"), "kind": "project",
                       "summary": format!("Note number {i} about the release"),
                       "body": "The release checklist and its rollback plan. ".repeat(24),
                       "project": "/w/m"}),
            )
            .unwrap();
    }
}

fn snapshot_commits(runtime: &StoreRuntime) -> Vec<u64> {
    runtime
        .store()
        .snapshots()
        .unwrap()
        .iter()
        .map(|snapshot| snapshot.commit_seq)
        .collect()
}

fn slugs(runtime: &StoreRuntime) -> Value {
    runtime
        .call("memories", json!({"k": 200, "project": "/w/m"}))
        .unwrap()
}

#[test]
fn a_store_checkpoints_by_itself_at_the_threshold() {
    // A new store starts with the catalog in its WAL (the schema and every memory function),
    // which grows with each catalog change. So the threshold is not a constant: the store is
    // opened once with automatic checkpoints off to measure that WAL, then again with a
    // threshold of that WAL plus a margin; the open stays under it, the test's own writes
    // (about 1 KiB each, 120 of them) go clearly over it.
    let data = TempDir::new("ckpt");
    let off = Settings {
        checkpoint_after: 0,
        keep_history: None,
    };
    let runtime = open(&data, &off);
    let catalog_wal = runtime.maintenance_status()["wal_since_snapshot"]
        .as_u64()
        .unwrap();
    runtime.close().unwrap();
    let threshold = catalog_wal + (16 << 10);
    let settings = Settings {
        checkpoint_after: threshold,
        keep_history: None,
    };
    let runtime = open(&data, &settings);
    assert_eq!(
        snapshot_commits(&runtime),
        vec![0],
        "no automatic checkpoint right after the open (catalog WAL {catalog_wal} bytes)"
    );
    write(&runtime, 0, 120);
    let deadline = Instant::now() + Duration::from_secs(60);
    while snapshot_commits(&runtime).len() < 2 {
        assert!(
            Instant::now() < deadline,
            "no automatic checkpoint was written"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = runtime.maintenance_status();
    assert_eq!(status["checkpoint_after"], threshold, "{status}");
    assert!(status["snapshots"].as_u64().unwrap() >= 2, "{status}");
    assert!(status["size"].as_u64().unwrap() > 0, "{status}");
    let lines = maintenance::status_lines("default", &status).join("\n");
    assert!(lines.contains("WAL since the last snapshot"), "{lines}");
    let formatted = format!(
        "checkpoint after {}",
        crate::download::human_bytes(threshold)
    );
    assert!(lines.contains(&formatted), "{formatted}: {lines}");
    assert!(lines.contains("history kept: forever"), "{lines}");
    runtime.close().unwrap();

    // The default threshold (16 MiB) is not reached by the same writes.
    let data = TempDir::new("ckpt-default");
    let runtime = open(&data, &Settings::default());
    write(&runtime, 0, 120);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(snapshot_commits(&runtime), vec![0]);
    runtime.close().unwrap();
}

#[test]
fn a_prune_keeps_the_last_days_and_waits_for_a_copy() {
    let data = TempDir::new("prune");
    let keep = Duration::from_secs(30 * 86_400);
    let settings = Settings {
        checkpoint_after: 0,
        keep_history: Some(keep),
    };
    let runtime = open(&data, &settings);
    let mut times = Vec::new();
    for round in 0..3 {
        write(&runtime, round * 10, 10);
        let snapshot = runtime.store().checkpoint(None).unwrap();
        times.push((snapshot.commit_seq, snapshot.last_commit_at));
        std::thread::sleep(Duration::from_millis(5));
    }
    write(&runtime, 30, 5);
    let [a, b, c] = [times[0], times[1], times[2]];
    assert_eq!(snapshot_commits(&runtime), vec![0, a.0, b.0, c.0]);

    // Today nothing is 30 days old.
    let now = maintenance::now_micros();
    assert_eq!(runtime.prune_old_history(keep, now).unwrap(), None);
    assert_eq!(snapshot_commits(&runtime), vec![0, a.0, b.0, c.0]);

    // 30 days after a moment between the snapshots a and b: every point from that moment on
    // stays reachable, so a (the newest snapshot before it) stays and only 0 goes.
    let keep_us = i64::try_from(keep.as_micros()).unwrap();
    let between = (a.1 + b.1) / 2 + keep_us;
    let store_dir = runtime.dir().to_path_buf();
    // A copy in progress (the bench's fork) holds the copy lock: the prune waits for it.
    let refused =
        maintenance::hold_for_copy(&store_dir, || Ok(runtime.prune_old_history(keep, between)))
            .unwrap();
    let err = refused.unwrap_err();
    assert!(err.contains("copy"), "{err}");
    assert_eq!(snapshot_commits(&runtime), vec![0, a.0, b.0, c.0]);

    let pruned = runtime.prune_old_history(keep, between).unwrap().unwrap();
    assert_eq!(pruned.up_to, a.0, "{pruned:?}");
    assert_eq!(pruned.kept_from, a.0, "{pruned:?}");
    assert_eq!(pruned.snapshots, 1, "{pruned:?}");
    assert!(pruned.text().contains("1 snapshot(s)"), "{}", pruned.text());
    assert_eq!(snapshot_commits(&runtime), vec![a.0, b.0, c.0]);
    // Again at the same moment: nothing left to prune.
    assert_eq!(runtime.prune_old_history(keep, between).unwrap(), None);

    // Far in the future: the engine still keeps the last two snapshots (repair donors).
    let later = c.1 + keep_us * 10;
    runtime.prune_old_history(keep, later).unwrap().unwrap();
    assert_eq!(snapshot_commits(&runtime), vec![b.0, c.0]);
    // Every memory is still there: a prune removes history, never the current state.
    let listed = slugs(&runtime).to_string();
    for i in [0, 15, 34] {
        assert!(listed.contains(&format!("note-{i}")), "{listed}");
    }
    runtime.close().unwrap();
}

#[test]
fn a_bench_fork_still_opens_after_a_prune() {
    let data = TempDir::new("forkprune");
    let keep = Duration::from_secs(86_400);
    let settings = Settings {
        checkpoint_after: 0,
        keep_history: Some(keep),
    };
    let runtime = open(&data, &settings);
    for round in 0..3 {
        write(&runtime, round * 5, 5);
        runtime.store().checkpoint(None).unwrap();
    }
    write(&runtime, 15, 2);
    // The bench's copy, made while the daemon serves the store.
    let copy = TempDir::new("forkprune-copy");
    let fork_dir = crate::store::default_store_dir(copy.path());
    crate::bench::arms::fork_store(runtime.dir(), &fork_dir).unwrap();
    let later = maintenance::now_micros() + i64::try_from(keep.as_micros()).unwrap() * 3;
    assert!(runtime.prune_old_history(keep, later).unwrap().is_some());
    runtime.close().unwrap();

    let fork = open(&copy, &Settings::default());
    let listed = slugs(&fork).to_string();
    for i in [0, 9, 16] {
        assert!(listed.contains(&format!("note-{i}")), "{listed}");
    }
    fork.close().unwrap();
}

#[test]
fn files_status_reads_a_closed_store() {
    let data = TempDir::new("fstatus");
    let runtime = open(&data, &Settings::default());
    write(&runtime, 0, 3);
    let dir = runtime.dir().to_path_buf();
    runtime.close().unwrap();
    let status = maintenance::files_status(&dir, &Settings::default());
    assert_eq!(status["snapshots"], 1, "{status}");
    assert!(status["wal_bytes"].as_u64().unwrap() > 0, "{status}");
    assert!(status.get("wal_since_snapshot").is_none(), "{status}");
    let lines = maintenance::status_lines("default", &status).join("\n");
    assert!(lines.contains("known while the daemon runs"), "{lines}");
    assert!(lines.contains("history kept: 30 days"), "{lines}");
    assert!(lines.contains("last prune: never"), "{lines}");
    // A WAL far above the threshold is a doctor problem; within it, fine.
    let mut open_status = status.clone();
    open_status["wal_since_snapshot"] = json!(70u64 << 20);
    let finding = maintenance::wal_finding("default", &open_status).unwrap();
    assert!(finding.problem, "{}", finding.text);
    assert!(
        finding.text.contains("every daemon start replays it"),
        "{}",
        finding.text
    );
    open_status["wal_since_snapshot"] = json!(1u64 << 20);
    assert!(
        !maintenance::wal_finding("default", &open_status)
            .unwrap()
            .problem
    );
    assert!(maintenance::wal_finding("default", &status).is_none());
}
