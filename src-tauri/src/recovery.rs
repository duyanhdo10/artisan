use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tempfile::Builder as TempFileBuilder;

#[cfg(not(windows))]
use std::os::unix::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

use super::{
    hash_bytes, is_retryable_replace_error, platform_replace_file, read_markdown_note,
    validate_content_hash, validate_relative_note_path, CleanupPath, CommandError,
    ReplacementReceipt, REPLACE_RETRY_DELAYS_MS,
};

const RECOVERY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
struct StoredRecoveryDraft {
    schema_version: u32,
    vault_id: String,
    relative_path: String,
    base_hash: String,
    content_hash: String,
    editor_revision: u64,
    updated_at_unix_ms: u64,
    content: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveryDraftSummary {
    relative_path: String,
    base_hash: String,
    content_hash: String,
    editor_revision: u64,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveryDraft {
    relative_path: String,
    base_hash: String,
    content_hash: String,
    editor_revision: u64,
    updated_at_unix_ms: u64,
    content: String,
}

struct RecoveryDraftInput<'a> {
    relative_path: &'a str,
    content: &'a str,
    base_hash: &'a str,
    editor_revision: u64,
    expected_draft_hash: Option<&'a str>,
}

pub(crate) fn write_recovery_draft(
    app_local_data_root: &Path,
    vault_root: &Path,
    relative_path: &str,
    content: &str,
    base_hash: &str,
    editor_revision: u64,
    expected_draft_hash: Option<&str>,
) -> Result<RecoveryDraftSummary, CommandError> {
    write_recovery_draft_with(
        app_local_data_root,
        vault_root,
        RecoveryDraftInput {
            relative_path,
            content,
            base_hash,
            editor_revision,
            expected_draft_hash,
        },
        platform_replace_file,
    )
}

fn write_recovery_draft_with<F>(
    app_local_data_root: &Path,
    vault_root: &Path,
    input: RecoveryDraftInput<'_>,
    replace_file: F,
) -> Result<RecoveryDraftSummary, CommandError>
where
    F: Fn(&Path, &Path) -> io::Result<ReplacementReceipt>,
{
    validate_content_hash(input.base_hash)?;
    if let Some(expected_hash) = input.expected_draft_hash {
        validate_content_hash(expected_hash)?;
    }
    let normalized_path = normalize_relative_note_path(input.relative_path)?;
    let opened = read_markdown_note(vault_root, &normalized_path)?;
    if !opened.content_hash.eq_ignore_ascii_case(input.base_hash) {
        return Err(CommandError::new(
            "recovery_base_changed",
            "The note changed before its recovery draft could be protected.",
        ));
    }
    let vault_id = vault_id(vault_root);
    let note_id = hash_bytes(normalized_path.as_bytes());
    let recovery_dir = recovery_directory(app_local_data_root, &vault_id);
    fs::create_dir_all(&recovery_dir).map_err(|_| {
        CommandError::new(
            "recovery_prepare_failed",
            "The recovery draft directory could not be prepared.",
        )
    })?;

    let updated_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CommandError::internal())?
        .as_millis()
        .try_into()
        .map_err(|_| CommandError::internal())?;
    let stored = StoredRecoveryDraft {
        schema_version: RECOVERY_SCHEMA_VERSION,
        vault_id,
        relative_path: normalized_path,
        base_hash: input.base_hash.to_ascii_lowercase(),
        content_hash: hash_bytes(input.content.as_bytes()),
        editor_revision: input.editor_revision,
        updated_at_unix_ms,
        content: input.content.to_owned(),
    };
    let encoded = serde_json::to_vec(&stored).map_err(|_| CommandError::internal())?;
    let target = recovery_dir.join(format!("{note_id}.json"));
    validate_draft_precondition(
        &target,
        &stored.vault_id,
        &note_id,
        input.expected_draft_hash,
    )?;

    let mut temporary = TempFileBuilder::new()
        .prefix(".astian-recovery-")
        .suffix(".tmp")
        .tempfile_in(&recovery_dir)
        .map_err(|_| {
            CommandError::new(
                "recovery_prepare_failed",
                "The recovery draft could not be prepared.",
            )
        })?;
    temporary
        .write_all(&encoded)
        .and_then(|_| temporary.flush())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| {
            CommandError::new(
                "recovery_write_failed",
                "The recovery draft could not be written safely.",
            )
        })?;
    let (temporary_file, temporary_path) = temporary.keep().map_err(|_| {
        CommandError::new(
            "recovery_prepare_failed",
            "The recovery draft could not be prepared.",
        )
    })?;
    drop(temporary_file);
    let _temporary_cleanup = CleanupPath(temporary_path.clone());

    let receipt = install_recovery_file(
        &target,
        &temporary_path,
        &stored.vault_id,
        &note_id,
        input.expected_draft_hash,
        &replace_file,
    )?;
    let verified = fs::read(&target).map_err(|_| {
        CommandError::new(
            "recovery_verification_failed",
            "Astian could not verify the recovery draft.",
        )
    })?;
    if verified != encoded {
        return Err(CommandError::new(
            "recovery_verification_failed",
            "Astian could not verify the recovery draft.",
        ));
    }
    if let Some(recovery_path) = receipt.and_then(|value| value.recovery_path) {
        let _ = fs::remove_file(recovery_path);
    }

    Ok(summary_from(&stored))
}

fn install_recovery_file<F>(
    target: &Path,
    temporary_path: &Path,
    expected_vault_id: &str,
    expected_note_id: &str,
    expected_draft_hash: Option<&str>,
    replace_file: &F,
) -> Result<Option<ReplacementReceipt>, CommandError>
where
    F: Fn(&Path, &Path) -> io::Result<ReplacementReceipt>,
{
    if expected_draft_hash.is_none() {
        match fs::hard_link(temporary_path, target) {
            Ok(()) => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(CommandError::new(
                    "recovery_exists",
                    "A previous recovery draft must be reviewed before editing this note.",
                ));
            }
            Err(_) => {
                return Err(CommandError::new(
                    "recovery_replace_failed",
                    "The recovery draft could not be installed safely.",
                ));
            }
        }
    }

    for delay_ms in REPLACE_RETRY_DELAYS_MS {
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms));
        }
        validate_draft_precondition(
            target,
            expected_vault_id,
            expected_note_id,
            expected_draft_hash,
        )?;
        match replace_file(target, temporary_path) {
            Ok(receipt) => return Ok(Some(receipt)),
            Err(error) if is_retryable_replace_error(&error) => continue,
            Err(_) => {
                return Err(CommandError::new(
                    "recovery_replace_failed",
                    "The recovery draft could not be replaced safely.",
                ));
            }
        }
    }

    Err(CommandError::new(
        "recovery_locked",
        "Another program is using the recovery draft. The previous draft was preserved.",
    ))
}

fn validate_draft_precondition(
    target: &Path,
    expected_vault_id: &str,
    expected_note_id: &str,
    expected_draft_hash: Option<&str>,
) -> Result<(), CommandError> {
    let exists = target.try_exists().map_err(|_| {
        CommandError::new(
            "recovery_read_failed",
            "The recovery draft could not be inspected.",
        )
    })?;
    match (exists, expected_draft_hash) {
        (false, None) => Ok(()),
        (true, None) => Err(CommandError::new(
            "recovery_exists",
            "A previous recovery draft must be reviewed before editing this note.",
        )),
        (false, Some(_)) => Err(CommandError::new(
            "recovery_changed",
            "The recovery draft changed before it could be updated.",
        )),
        (true, Some(expected_hash)) => {
            let current = read_and_validate(target, expected_vault_id, Some(expected_note_id))?;
            if current.content_hash.eq_ignore_ascii_case(expected_hash) {
                Ok(())
            } else {
                Err(CommandError::new(
                    "recovery_changed",
                    "The recovery draft changed before it could be updated.",
                ))
            }
        }
    }
}

pub(crate) fn list_recovery_drafts(
    app_local_data_root: &Path,
    vault_root: &Path,
) -> Result<Vec<RecoveryDraftSummary>, CommandError> {
    let expected_vault_id = vault_id(vault_root);
    let recovery_dir = recovery_directory(app_local_data_root, &expected_vault_id);
    let entries = match fs::read_dir(recovery_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => {
            return Err(CommandError::new(
                "recovery_read_failed",
                "Recovery drafts could not be listed.",
            ));
        }
    };

    let mut drafts = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| {
            CommandError::new(
                "recovery_read_failed",
                "Recovery drafts could not be listed.",
            )
        })?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let stored = read_and_validate(&path, &expected_vault_id, None)?;
        drafts.push(summary_from(&stored));
    }

    drafts.sort_by(|left, right| {
        right
            .updated_at_unix_ms
            .cmp(&left.updated_at_unix_ms)
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });
    Ok(drafts)
}

pub(crate) fn read_recovery_draft(
    app_local_data_root: &Path,
    vault_root: &Path,
    relative_path: &str,
) -> Result<RecoveryDraft, CommandError> {
    let normalized_path = normalize_relative_note_path(relative_path)?;
    let expected_vault_id = vault_id(vault_root);
    let note_id = hash_bytes(normalized_path.as_bytes());
    let path =
        recovery_directory(app_local_data_root, &expected_vault_id).join(format!("{note_id}.json"));
    let stored = read_and_validate(&path, &expected_vault_id, Some(&note_id))?;
    if stored.relative_path != normalized_path {
        return Err(recovery_corrupt());
    }

    Ok(RecoveryDraft {
        relative_path: stored.relative_path,
        base_hash: stored.base_hash,
        content_hash: stored.content_hash,
        editor_revision: stored.editor_revision,
        updated_at_unix_ms: stored.updated_at_unix_ms,
        content: stored.content,
    })
}

pub(crate) fn clear_recovery_draft(
    app_local_data_root: &Path,
    vault_root: &Path,
    relative_path: &str,
) -> Result<(), CommandError> {
    let normalized_path = normalize_relative_note_path(relative_path)?;
    let target = draft_path(app_local_data_root, vault_root, &normalized_path);
    match fs::remove_file(target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(CommandError::new(
            "recovery_cleanup_failed",
            "The recovery draft could not be removed.",
        )),
    }
}

pub(crate) fn clear_recovery_draft_if_content_matches(
    app_local_data_root: &Path,
    vault_root: &Path,
    relative_path: &str,
    content: &str,
) -> Result<(), CommandError> {
    let normalized_path = normalize_relative_note_path(relative_path)?;
    let expected_vault_id = vault_id(vault_root);
    let target = draft_path(app_local_data_root, vault_root, &normalized_path);
    if !target.try_exists().map_err(|_| {
        CommandError::new(
            "recovery_read_failed",
            "The recovery draft could not be inspected.",
        )
    })? {
        return Ok(());
    }

    let note_id = hash_bytes(normalized_path.as_bytes());
    let stored = read_and_validate(&target, &expected_vault_id, Some(&note_id))?;
    if stored.relative_path == normalized_path
        && stored.content_hash == hash_bytes(content.as_bytes())
    {
        clear_recovery_draft(app_local_data_root, vault_root, &normalized_path)?;
    }
    Ok(())
}

fn read_and_validate(
    path: &Path,
    expected_vault_id: &str,
    expected_note_id: Option<&str>,
) -> Result<StoredRecoveryDraft, CommandError> {
    let bytes = fs::read(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            CommandError::new("recovery_not_found", "The recovery draft is unavailable.")
        } else {
            CommandError::new(
                "recovery_read_failed",
                "The recovery draft could not be read.",
            )
        }
    })?;
    let stored: StoredRecoveryDraft =
        serde_json::from_slice(&bytes).map_err(|_| recovery_corrupt())?;

    if stored.schema_version != RECOVERY_SCHEMA_VERSION {
        return Err(CommandError::new(
            "recovery_unsupported",
            "The recovery draft uses an unsupported format.",
        ));
    }
    validate_content_hash(&stored.base_hash).map_err(|_| recovery_corrupt())?;
    validate_content_hash(&stored.content_hash).map_err(|_| recovery_corrupt())?;
    let normalized_path =
        normalize_relative_note_path(&stored.relative_path).map_err(|_| recovery_corrupt())?;
    let actual_note_id = hash_bytes(normalized_path.as_bytes());
    let file_note_id = path.file_stem().and_then(|value| value.to_str());
    if stored.vault_id != expected_vault_id
        || stored.relative_path != normalized_path
        || stored.content_hash != hash_bytes(stored.content.as_bytes())
        || file_note_id != Some(actual_note_id.as_str())
        || expected_note_id.is_some_and(|expected| expected != actual_note_id)
    {
        return Err(recovery_corrupt());
    }

    Ok(stored)
}

fn normalize_relative_note_path(relative_path: &str) -> Result<String, CommandError> {
    let path = validate_relative_note_path(relative_path)?;
    Ok(path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn vault_id(vault_root: &Path) -> String {
    hash_bytes(&vault_identity_bytes(vault_root))
}

#[cfg(windows)]
fn vault_identity_bytes(vault_root: &Path) -> Vec<u8> {
    vault_root
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(not(windows))]
fn vault_identity_bytes(vault_root: &Path) -> Vec<u8> {
    vault_root.as_os_str().as_bytes().to_vec()
}

fn recovery_directory(app_local_data_root: &Path, vault_id: &str) -> PathBuf {
    app_local_data_root
        .join("vaults")
        .join(vault_id)
        .join("recovery")
}

fn draft_path(app_local_data_root: &Path, vault_root: &Path, normalized_path: &str) -> PathBuf {
    recovery_directory(app_local_data_root, &vault_id(vault_root))
        .join(format!("{}.json", hash_bytes(normalized_path.as_bytes())))
}

fn summary_from(stored: &StoredRecoveryDraft) -> RecoveryDraftSummary {
    RecoveryDraftSummary {
        relative_path: stored.relative_path.clone(),
        base_hash: stored.base_hash.clone(),
        content_hash: stored.content_hash.clone(),
        editor_revision: stored.editor_revision,
        updated_at_unix_ms: stored.updated_at_unix_ms,
    }
}

fn recovery_corrupt() -> CommandError {
    CommandError::new(
        "recovery_corrupt",
        "The recovery draft is invalid and was left unchanged.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn roots() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().expect("temporary root should be created");
        let app_data = temp.path().join("app-data");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("vault should be created");
        fs::write(vault.join("note.md"), b"saved").expect("base note should be written");
        let vault = fs::canonicalize(vault).expect("vault should canonicalize");
        (temp, app_data, vault)
    }

    #[test]
    fn draft_round_trip_stays_outside_vault_and_hides_note_name() {
        let (_temp, app_data, vault) = roots();
        let base_hash = hash_bytes(b"saved");
        let relative_path = "ghi-chú/Rất riêng tư.md";
        let content = "# Bản nháp\nNội dung chưa lưu\n";
        fs::create_dir_all(vault.join("ghi-chú")).expect("nested note folder should be created");
        fs::write(vault.join(relative_path), b"saved").expect("base note should be written");

        let summary = write_recovery_draft(
            &app_data,
            &vault,
            relative_path,
            content,
            &base_hash,
            7,
            None,
        )
        .expect("draft should be written");
        let drafts = list_recovery_drafts(&app_data, &vault).expect("drafts should list");
        let restored =
            read_recovery_draft(&app_data, &vault, relative_path).expect("draft should read");

        assert_eq!(drafts, vec![summary]);
        let metadata = serde_json::to_value(&drafts[0]).expect("metadata should serialize");
        assert!(metadata.get("content").is_none());
        assert_eq!(restored.content, content);
        assert_eq!(restored.editor_revision, 7);
        assert!(vault.join(relative_path).is_file());
        assert!(!vault.join("vaults").exists());
        let filenames: Vec<_> = fs::read_dir(recovery_directory(&app_data, &vault_id(&vault)))
            .expect("recovery directory should exist")
            .map(|entry| entry.expect("entry should read").file_name())
            .collect();
        assert_eq!(filenames.len(), 1);
        assert!(!filenames[0].to_string_lossy().contains("Rất riêng tư"));
    }

    #[test]
    fn rewrite_replaces_latest_draft_and_cleans_internal_artifacts() {
        let (_temp, app_data, vault) = roots();
        let base_hash = hash_bytes(b"saved");
        write_recovery_draft(&app_data, &vault, "note.md", "first", &base_hash, 1, None)
            .expect("first draft should write");
        let first_hash = hash_bytes(b"first");
        write_recovery_draft(
            &app_data,
            &vault,
            "note.md",
            "second",
            &base_hash,
            2,
            Some(&first_hash),
        )
        .expect("second draft should replace first");

        let restored =
            read_recovery_draft(&app_data, &vault, "note.md").expect("draft should read");
        assert_eq!(restored.content, "second");
        assert_eq!(restored.editor_revision, 2);
        let recovery_dir = recovery_directory(&app_data, &vault_id(&vault));
        assert!(fs::read_dir(recovery_dir)
            .expect("directory should read")
            .all(|entry| !entry
                .expect("entry should read")
                .file_name()
                .to_string_lossy()
                .starts_with(".astian-")));
    }

    #[test]
    fn corrupt_draft_is_reported_and_preserved() {
        let (_temp, app_data, vault) = roots();
        let base_hash = hash_bytes(b"saved");
        write_recovery_draft(&app_data, &vault, "note.md", "draft", &base_hash, 1, None)
            .expect("draft should write");
        let target = draft_path(&app_data, &vault, "note.md");
        fs::write(&target, b"not-json").expect("fixture should be corrupted");

        let error = list_recovery_drafts(&app_data, &vault)
            .expect_err("corrupt draft should fail validation");
        assert_eq!(error.code, "recovery_corrupt");
        assert_eq!(
            fs::read(&target).expect("corrupt file should remain"),
            b"not-json"
        );
    }

    #[test]
    fn previous_session_draft_requires_review_before_replacement() {
        let (_temp, app_data, vault) = roots();
        let base_hash = hash_bytes(b"saved");
        write_recovery_draft(
            &app_data,
            &vault,
            "note.md",
            "previous session",
            &base_hash,
            1,
            None,
        )
        .expect("previous draft should write");

        let error = write_recovery_draft(
            &app_data,
            &vault,
            "note.md",
            "new session",
            &base_hash,
            1,
            None,
        )
        .expect_err("unclaimed draft must not be overwritten");

        let mismatch = write_recovery_draft(
            &app_data,
            &vault,
            "note.md",
            "new session",
            &base_hash,
            2,
            Some(&hash_bytes(b"wrong draft")),
        )
        .expect_err("mismatched draft hash must not replace recovery");

        assert_eq!(error.code, "recovery_exists");
        assert_eq!(mismatch.code, "recovery_changed");
        assert_eq!(
            read_recovery_draft(&app_data, &vault, "note.md")
                .expect("previous draft should remain")
                .content,
            "previous session"
        );
    }

    #[test]
    fn unsupported_schema_is_reported_and_preserved() {
        let (_temp, app_data, vault) = roots();
        let base_hash = hash_bytes(b"saved");
        write_recovery_draft(&app_data, &vault, "note.md", "draft", &base_hash, 1, None)
            .expect("draft should write");
        let target = draft_path(&app_data, &vault, "note.md");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&target).expect("draft fixture should read"))
                .expect("draft fixture should parse");
        value["schema_version"] = serde_json::json!(RECOVERY_SCHEMA_VERSION + 1);
        let unsupported = serde_json::to_vec(&value).expect("fixture should serialize");
        fs::write(&target, &unsupported).expect("fixture should be updated");

        let error = list_recovery_drafts(&app_data, &vault)
            .expect_err("unsupported draft should fail validation");

        assert_eq!(error.code, "recovery_unsupported");
        assert_eq!(
            fs::read(&target).expect("unsupported draft should remain"),
            unsupported
        );
    }

    #[test]
    fn failed_rewrite_preserves_previous_draft_and_cleans_temporary_file() {
        let (_temp, app_data, vault) = roots();
        let base_hash = hash_bytes(b"saved");
        write_recovery_draft(&app_data, &vault, "note.md", "first", &base_hash, 1, None)
            .expect("first draft should write");
        let calls = Cell::new(0);

        let error = write_recovery_draft_with(
            &app_data,
            &vault,
            RecoveryDraftInput {
                relative_path: "note.md",
                content: "second",
                base_hash: &base_hash,
                editor_revision: 2,
                expected_draft_hash: Some(&hash_bytes(b"first")),
            },
            |_, _| {
                calls.set(calls.get() + 1);
                Err(io::Error::from_raw_os_error(5))
            },
        )
        .expect_err("locked rewrite should fail");

        assert_eq!(error.code, "recovery_locked");
        assert_eq!(calls.get(), REPLACE_RETRY_DELAYS_MS.len());
        assert_eq!(
            read_recovery_draft(&app_data, &vault, "note.md")
                .expect("old draft should remain")
                .content,
            "first"
        );
        let recovery_dir = recovery_directory(&app_data, &vault_id(&vault));
        assert!(fs::read_dir(recovery_dir)
            .expect("directory should read")
            .all(|entry| !entry
                .expect("entry should read")
                .file_name()
                .to_string_lossy()
                .starts_with(".astian-")));
    }

    #[test]
    fn matching_cleanup_is_safe_and_explicit_cleanup_is_idempotent() {
        let (_temp, app_data, vault) = roots();
        let base_hash = hash_bytes(b"saved");
        write_recovery_draft(&app_data, &vault, "note.md", "draft", &base_hash, 1, None)
            .expect("draft should write");

        clear_recovery_draft_if_content_matches(&app_data, &vault, "note.md", "other")
            .expect("mismatched cleanup should be harmless");
        assert!(draft_path(&app_data, &vault, "note.md").exists());
        clear_recovery_draft_if_content_matches(&app_data, &vault, "note.md", "draft")
            .expect("matching cleanup should remove draft");
        clear_recovery_draft(&app_data, &vault, "note.md")
            .expect("explicit cleanup should be idempotent");
        assert!(!draft_path(&app_data, &vault, "note.md").exists());
    }

    #[test]
    fn traversal_and_invalid_hash_are_rejected_before_writing() {
        let (_temp, app_data, vault) = roots();
        let invalid_path = write_recovery_draft(
            &app_data,
            &vault,
            "../outside.md",
            "draft",
            &hash_bytes(b"saved"),
            1,
            None,
        )
        .expect_err("traversal should fail");
        let invalid_hash =
            write_recovery_draft(&app_data, &vault, "note.md", "draft", "not-a-hash", 1, None)
                .expect_err("invalid base hash should fail");

        assert_eq!(invalid_path.code, "invalid_note_path");
        assert_eq!(invalid_hash.code, "invalid_content_hash");
        assert!(!app_data.exists());
    }

    #[test]
    fn changed_disk_base_rejects_a_late_draft_without_overwriting_the_latest() {
        let (_temp, app_data, vault) = roots();
        let saved_hash = hash_bytes(b"saved");
        write_recovery_draft(
            &app_data,
            &vault,
            "note.md",
            "protected",
            &saved_hash,
            1,
            None,
        )
        .expect("initial draft should write");
        fs::write(vault.join("note.md"), b"new saved value").expect("disk note should change");

        let error = write_recovery_draft(
            &app_data,
            &vault,
            "note.md",
            "late stale draft",
            &saved_hash,
            2,
            Some(&hash_bytes(b"protected")),
        )
        .expect_err("late draft should be rejected");

        assert_eq!(error.code, "recovery_base_changed");
        assert_eq!(
            read_recovery_draft(&app_data, &vault, "note.md")
                .expect("previous draft should remain")
                .content,
            "protected"
        );
    }
}
