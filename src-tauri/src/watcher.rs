use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use tauri::{AppHandle, Emitter};

use super::{list_markdown_notes, read_markdown_note, CommandError, NoteEntry};

pub(crate) const VAULT_CHANGED_EVENT: &str = "vault://changed";
const WATCH_DEBOUNCE_MS: u64 = 150;
const RESCAN_RETRY_DELAYS_MS: [u64; 3] = [0, 75, 250];

pub(crate) type ExpectedWrites = Arc<Mutex<HashMap<String, String>>>;
type VaultSnapshot = BTreeMap<String, String>;

#[derive(Clone, Copy)]
enum WatchSignal {
    Activity,
    Error,
    Stop,
}

pub(crate) struct VaultWatcher {
    _watcher: RecommendedWatcher,
    signal_tx: Sender<WatchSignal>,
    worker: Option<JoinHandle<()>>,
}

impl VaultWatcher {
    pub(crate) fn request_reconcile(&self) -> Result<(), CommandError> {
        self.signal_tx.send(WatchSignal::Activity).map_err(|_| {
            CommandError::new("watcher_unavailable", "The vault watcher is unavailable.")
        })
    }
}

impl Drop for VaultWatcher {
    fn drop(&mut self) {
        let _ = self.signal_tx.send(WatchSignal::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VaultChangeKind {
    Created,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VaultChangeSource {
    Astian,
    External,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VaultChange {
    kind: VaultChangeKind,
    source: VaultChangeSource,
    relative_path: String,
    previous_relative_path: Option<String>,
    content_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum VaultWatcherStatus {
    Changed,
    RescanRequired,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VaultWatcherEvent {
    vault_session: u64,
    revision: u64,
    status: VaultWatcherStatus,
    error_code: Option<&'static str>,
    changes: Vec<VaultChange>,
    notes: Vec<NoteEntry>,
}

pub(crate) fn start_vault_watcher(
    app: AppHandle,
    root: PathBuf,
    vault_session: u64,
    expected_writes: ExpectedWrites,
    write_lock: Arc<Mutex<()>>,
) -> Result<VaultWatcher, CommandError> {
    let (signal_tx, signal_rx) = mpsc::channel();
    let callback_tx = signal_tx.clone();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let signal = if result.is_ok() {
            WatchSignal::Activity
        } else {
            WatchSignal::Error
        };
        let _ = callback_tx.send(signal);
    })
    .map_err(|_| {
        CommandError::new(
            "watcher_unavailable",
            "The vault watcher could not be started.",
        )
    })?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|_| {
            CommandError::new(
                "watcher_unavailable",
                "The vault folder could not be watched.",
            )
        })?;

    let (initial_snapshot, _) = scan_snapshot(&root)?;
    let worker = thread::Builder::new()
        .name("astian-vault-watcher".to_owned())
        .spawn(move || {
            watcher_worker(
                app,
                root,
                vault_session,
                expected_writes,
                write_lock,
                initial_snapshot,
                signal_rx,
            )
        })
        .map_err(|_| {
            CommandError::new(
                "watcher_unavailable",
                "The vault watcher worker could not be started.",
            )
        })?;

    Ok(VaultWatcher {
        _watcher: watcher,
        signal_tx,
        worker: Some(worker),
    })
}

pub(crate) fn record_expected_write(
    expected_writes: &ExpectedWrites,
    relative_path: &str,
    content_hash: &str,
) -> Result<(), CommandError> {
    expected_writes
        .lock()
        .map_err(|_| CommandError::internal())?
        .insert(relative_path.to_owned(), content_hash.to_owned());
    Ok(())
}

fn watcher_worker(
    app: AppHandle,
    root: PathBuf,
    vault_session: u64,
    expected_writes: ExpectedWrites,
    write_lock: Arc<Mutex<()>>,
    mut snapshot: VaultSnapshot,
    signal_rx: Receiver<WatchSignal>,
) {
    let mut revision = 0_u64;
    while let Ok(signal) = signal_rx.recv() {
        match signal {
            WatchSignal::Stop => return,
            WatchSignal::Activity | WatchSignal::Error => {}
        }

        let mut backend_error = matches!(signal, WatchSignal::Error);
        loop {
            match signal_rx.recv_timeout(Duration::from_millis(WATCH_DEBOUNCE_MS)) {
                Ok(WatchSignal::Activity) => {}
                Ok(WatchSignal::Error) => backend_error = true,
                Ok(WatchSignal::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => break,
            }
        }

        revision = revision.saturating_add(1);
        let Ok(_write_guard) = write_lock.lock() else {
            let _ = app.emit(
                VAULT_CHANGED_EVENT,
                VaultWatcherEvent {
                    vault_session,
                    revision,
                    status: VaultWatcherStatus::RescanRequired,
                    error_code: Some("watcher_state_unavailable"),
                    changes: Vec::new(),
                    notes: Vec::new(),
                },
            );
            return;
        };
        let scanned = scan_snapshot_with_retries(&root);
        let Ok((next_snapshot, notes)) = scanned else {
            let _ = app.emit(
                VAULT_CHANGED_EVENT,
                VaultWatcherEvent {
                    vault_session,
                    revision,
                    status: VaultWatcherStatus::RescanRequired,
                    error_code: Some("watcher_rescan_required"),
                    changes: Vec::new(),
                    notes: Vec::new(),
                },
            );
            continue;
        };

        let changes = match expected_writes.lock() {
            Ok(mut expected) => reconcile_snapshots(&snapshot, &next_snapshot, &mut expected),
            Err(_) => {
                let _ = app.emit(
                    VAULT_CHANGED_EVENT,
                    VaultWatcherEvent {
                        vault_session,
                        revision,
                        status: VaultWatcherStatus::RescanRequired,
                        error_code: Some("watcher_state_unavailable"),
                        changes: Vec::new(),
                        notes: Vec::new(),
                    },
                );
                return;
            }
        };
        snapshot = next_snapshot;

        if changes.is_empty() && !backend_error {
            continue;
        }
        let _ = app.emit(
            VAULT_CHANGED_EVENT,
            VaultWatcherEvent {
                vault_session,
                revision,
                status: VaultWatcherStatus::Changed,
                error_code: backend_error.then_some("watcher_backend_event_error"),
                changes,
                notes,
            },
        );
    }
}

fn scan_snapshot_with_retries(
    root: &Path,
) -> Result<(VaultSnapshot, Vec<NoteEntry>), CommandError> {
    let mut last_error = None;
    for delay_ms in RESCAN_RETRY_DELAYS_MS {
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms));
        }
        match scan_snapshot(root) {
            Ok(snapshot) => return Ok(snapshot),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(CommandError::internal))
}

fn scan_snapshot(root: &Path) -> Result<(VaultSnapshot, Vec<NoteEntry>), CommandError> {
    let notes = list_markdown_notes(root)?;
    let mut snapshot = BTreeMap::new();
    for note in &notes {
        let opened = read_markdown_note(root, &note.relative_path)?;
        snapshot.insert(note.relative_path.clone(), opened.content_hash);
    }
    Ok((snapshot, notes))
}

fn reconcile_snapshots(
    previous: &VaultSnapshot,
    current: &VaultSnapshot,
    expected_writes: &mut HashMap<String, String>,
) -> Vec<VaultChange> {
    let removed: Vec<_> = previous
        .iter()
        .filter(|(path, _)| !current.contains_key(*path))
        .map(|(path, hash)| (path.clone(), hash.clone()))
        .collect();
    let created: Vec<_> = current
        .iter()
        .filter(|(path, _)| !previous.contains_key(*path))
        .map(|(path, hash)| (path.clone(), hash.clone()))
        .collect();

    let mut removed_by_hash: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut created_by_hash: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, (_, hash)) in removed.iter().enumerate() {
        removed_by_hash.entry(hash).or_default().push(index);
    }
    for (index, (_, hash)) in created.iter().enumerate() {
        created_by_hash.entry(hash).or_default().push(index);
    }

    let mut paired_removed = HashSet::new();
    let mut paired_created = HashSet::new();
    let mut changes = Vec::new();
    for (hash, removed_indexes) in &removed_by_hash {
        let Some(created_indexes) = created_by_hash.get(hash) else {
            continue;
        };
        if removed_indexes.len() == 1 && created_indexes.len() == 1 {
            let removed_index = removed_indexes[0];
            let created_index = created_indexes[0];
            paired_removed.insert(removed_index);
            paired_created.insert(created_index);
            let (previous_path, _) = &removed[removed_index];
            let (relative_path, content_hash) = &created[created_index];
            expected_writes.remove(previous_path);
            expected_writes.remove(relative_path);
            changes.push(VaultChange {
                kind: VaultChangeKind::Renamed,
                source: VaultChangeSource::External,
                relative_path: relative_path.clone(),
                previous_relative_path: Some(previous_path.clone()),
                content_hash: Some(content_hash.clone()),
            });
        }
    }

    for (index, (relative_path, _)) in removed.iter().enumerate() {
        if paired_removed.contains(&index) {
            continue;
        }
        expected_writes.remove(relative_path);
        changes.push(VaultChange {
            kind: VaultChangeKind::Deleted,
            source: VaultChangeSource::External,
            relative_path: relative_path.clone(),
            previous_relative_path: None,
            content_hash: None,
        });
    }
    for (index, (relative_path, content_hash)) in created.iter().enumerate() {
        if paired_created.contains(&index) {
            continue;
        }
        let source = classify_source(expected_writes, relative_path, content_hash);
        changes.push(VaultChange {
            kind: VaultChangeKind::Created,
            source,
            relative_path: relative_path.clone(),
            previous_relative_path: None,
            content_hash: Some(content_hash.clone()),
        });
    }
    for (relative_path, content_hash) in current {
        if previous
            .get(relative_path)
            .is_some_and(|previous_hash| previous_hash != content_hash)
        {
            let source = classify_source(expected_writes, relative_path, content_hash);
            changes.push(VaultChange {
                kind: VaultChangeKind::Modified,
                source,
                relative_path: relative_path.clone(),
                previous_relative_path: None,
                content_hash: Some(content_hash.clone()),
            });
        }
    }
    changes.sort_by(|left, right| {
        left.relative_path.cmp(&right.relative_path).then_with(|| {
            left.previous_relative_path
                .cmp(&right.previous_relative_path)
        })
    });
    changes
}

fn classify_source(
    expected_writes: &mut HashMap<String, String>,
    relative_path: &str,
    content_hash: &str,
) -> VaultChangeSource {
    match expected_writes.remove(relative_path) {
        Some(expected) if expected.eq_ignore_ascii_case(content_hash) => VaultChangeSource::Astian,
        _ => VaultChangeSource::External,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash_bytes;

    #[cfg(windows)]
    fn wait_for_backend_activity(
        receiver: &Receiver<notify::Result<notify::Event>>,
    ) -> notify::Event {
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("watcher should report filesystem activity")
            .expect("watcher activity should not be an error")
    }

    fn snapshot(entries: &[(&str, &str)]) -> VaultSnapshot {
        entries
            .iter()
            .map(|(path, content)| ((*path).to_owned(), hash_bytes(content.as_bytes())))
            .collect()
    }

    #[test]
    fn burst_reconcile_collapses_to_final_create_modify_and_delete() {
        let previous = snapshot(&[("deleted.md", "old"), ("changed.md", "before")]);
        let current = snapshot(&[("changed.md", "after"), ("created.md", "final")]);
        let mut expected = HashMap::new();

        let changes = reconcile_snapshots(&previous, &current, &mut expected);

        assert_eq!(changes.len(), 3);
        assert!(changes.iter().any(|change| {
            change.kind == VaultChangeKind::Created && change.relative_path == "created.md"
        }));
        assert!(changes.iter().any(|change| {
            change.kind == VaultChangeKind::Modified && change.relative_path == "changed.md"
        }));
        assert!(changes.iter().any(|change| {
            change.kind == VaultChangeKind::Deleted && change.relative_path == "deleted.md"
        }));
    }

    #[test]
    fn unique_content_pairs_unicode_and_case_only_renames() {
        for (from, to) in [
            ("Ghi chú.md", "Ghi Chú.md"),
            ("Dự án/Kế hoạch.md", "Dự án/Kế hoạch mới.md"),
        ] {
            let previous = snapshot(&[(from, "same")]);
            let current = snapshot(&[(to, "same")]);
            let changes = reconcile_snapshots(&previous, &current, &mut HashMap::new());

            assert_eq!(changes.len(), 1);
            assert_eq!(changes[0].kind, VaultChangeKind::Renamed);
            assert_eq!(changes[0].previous_relative_path.as_deref(), Some(from));
            assert_eq!(changes[0].relative_path, to);
        }
    }

    #[test]
    fn duplicate_content_is_not_guessed_as_a_rename() {
        let previous = snapshot(&[("one.md", "same"), ("two.md", "same")]);
        let current = snapshot(&[("three.md", "same"), ("four.md", "same")]);
        let changes = reconcile_snapshots(&previous, &current, &mut HashMap::new());

        assert_eq!(
            changes
                .iter()
                .filter(|change| change.kind == VaultChangeKind::Renamed)
                .count(),
            0
        );
        assert_eq!(changes.len(), 4);
    }

    #[test]
    fn expected_hash_classifies_only_the_matching_final_write_as_astian() {
        let previous = snapshot(&[("note.md", "before")]);
        let current = snapshot(&[("note.md", "after")]);
        let final_hash = current["note.md"].clone();
        let mut expected = HashMap::from([("note.md".to_owned(), final_hash)]);

        let internal = reconcile_snapshots(&previous, &current, &mut expected);
        assert_eq!(internal[0].source, VaultChangeSource::Astian);
        assert!(expected.is_empty());

        let mut stale = HashMap::from([("note.md".to_owned(), hash_bytes(b"other"))]);
        let external = reconcile_snapshots(&previous, &current, &mut stale);
        assert_eq!(external[0].source, VaultChangeSource::External);
        assert!(stale.is_empty());
    }

    #[test]
    fn watcher_event_contract_uses_relative_metadata_without_content() {
        let event = VaultWatcherEvent {
            vault_session: 3,
            revision: 7,
            status: VaultWatcherStatus::Changed,
            error_code: None,
            changes: vec![VaultChange {
                kind: VaultChangeKind::Modified,
                source: VaultChangeSource::External,
                relative_path: "Dự án/Ghi chú.md".to_owned(),
                previous_relative_path: None,
                content_hash: Some(hash_bytes(b"changed")),
            }],
            notes: vec![NoteEntry {
                relative_path: "Dự án/Ghi chú.md".to_owned(),
                title: "Ghi chú".to_owned(),
            }],
        };

        let serialized = serde_json::to_value(event).expect("event should serialize");
        assert_eq!(serialized["vaultSession"], 3);
        assert_eq!(serialized["revision"], 7);
        assert_eq!(serialized["changes"][0]["relativePath"], "Dự án/Ghi chú.md");
        assert!(serialized.get("root").is_none());
        assert!(serialized.get("content").is_none());
        assert!(serialized["changes"][0].get("content").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_backend_observes_unicode_modify_case_rename_and_delete() {
        let temp = tempfile::tempdir().expect("watch fixture should be created");
        let (sender, receiver) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(sender).expect("watcher should start");
        watcher
            .watch(temp.path(), RecursiveMode::Recursive)
            .expect("fixture should be watched");

        let original = temp.path().join("Ghi Chú.md");
        let renamed = temp.path().join("Ghi chú.md");
        std::fs::write(&original, b"created").expect("Unicode note should be created");
        let _ = wait_for_backend_activity(&receiver);
        while receiver.try_recv().is_ok() {}

        std::fs::write(&original, b"modified").expect("Unicode note should be modified");
        let _ = wait_for_backend_activity(&receiver);
        while receiver.try_recv().is_ok() {}

        std::fs::rename(&original, &renamed).expect("case-only rename should succeed");
        let _ = wait_for_backend_activity(&receiver);
        while receiver.try_recv().is_ok() {}

        std::fs::remove_file(&renamed).expect("renamed note should be deleted");
        let _ = wait_for_backend_activity(&receiver);
    }

    #[cfg(windows)]
    #[test]
    fn locked_note_requests_retry_without_changing_the_previous_snapshot() {
        use std::os::windows::fs::OpenOptionsExt;

        let temp = tempfile::tempdir().expect("lock fixture should be created");
        let note = temp.path().join("locked.md");
        std::fs::write(&note, b"content").expect("fixture should be written");
        let root = std::fs::canonicalize(temp.path()).expect("fixture should canonicalize");
        let (before, _) = scan_snapshot(&root).expect("initial snapshot should scan");
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&note)
            .expect("exclusive lock should open");

        let error = scan_snapshot(&root).expect_err("locked note should defer reconciliation");
        assert_eq!(error.code, "note_read_failed");
        assert_eq!(before["locked.md"], hash_bytes(b"content"));

        drop(lock);
        let (after, _) = scan_snapshot(&root).expect("scan should recover after unlock");
        assert_eq!(after, before);
    }
}
