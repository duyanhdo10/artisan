use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::Path,
    thread,
    time::Duration,
};
use tempfile::Builder as TempFileBuilder;

use super::{
    hash_bytes, is_retryable_replace_error, platform_replace_file, validate_relative_note_path,
    CleanupPath, CommandError, ReplacementReceipt, REPLACE_RETRY_DELAYS_MS,
};

const SESSION_SCHEMA_VERSION: u32 = 1;
const SESSION_FILE_NAME: &str = "session.json";
const MAX_VAULT_SESSIONS: usize = 10;

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredSession {
    schema_version: u32,
    vaults: Vec<StoredVaultSession>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredVaultSession {
    vault_id: String,
    active_note: String,
}

impl StoredSession {
    fn empty() -> Self {
        Self {
            schema_version: SESSION_SCHEMA_VERSION,
            vaults: Vec::new(),
        }
    }
}

pub(crate) fn active_note(
    app_local_data_root: &Path,
    canonical_vault_root: &Path,
) -> Result<Option<String>, CommandError> {
    let vault_id = vault_id(canonical_vault_root)?;
    let (session, _) = read_session(app_local_data_root)?;
    Ok(session
        .vaults
        .into_iter()
        .find(|entry| entry.vault_id == vault_id)
        .map(|entry| entry.active_note))
}

pub(crate) fn remember_active_note(
    app_local_data_root: &Path,
    canonical_vault_root: &Path,
    relative_path: Option<&str>,
) -> Result<(), CommandError> {
    remember_active_note_with(
        app_local_data_root,
        canonical_vault_root,
        relative_path,
        platform_replace_file,
    )
}

fn remember_active_note_with<F>(
    app_local_data_root: &Path,
    canonical_vault_root: &Path,
    relative_path: Option<&str>,
    replace_file: F,
) -> Result<(), CommandError>
where
    F: Fn(&Path, &Path) -> io::Result<ReplacementReceipt>,
{
    let vault_id = vault_id(canonical_vault_root)?;
    if let Some(relative_path) = relative_path {
        validate_relative_note_path(relative_path)?;
    }
    let (mut session, expected_bytes) = read_session(app_local_data_root)?;
    let previous_vaults = session.vaults;
    let mut vaults: Vec<_> = previous_vaults
        .iter()
        .filter(|entry| entry.vault_id != vault_id)
        .map(|entry| StoredVaultSession {
            vault_id: entry.vault_id.clone(),
            active_note: entry.active_note.clone(),
        })
        .collect();
    if let Some(active_note) = relative_path {
        vaults.insert(
            0,
            StoredVaultSession {
                vault_id,
                active_note: active_note.to_owned(),
            },
        );
        vaults.truncate(MAX_VAULT_SESSIONS);
    }
    if vaults == previous_vaults {
        return Ok(());
    }
    session.vaults = vaults;
    write_session(
        app_local_data_root,
        &session,
        expected_bytes.as_deref(),
        replace_file,
    )
}

fn vault_id(canonical_vault_root: &Path) -> Result<String, CommandError> {
    if !canonical_vault_root.is_absolute() {
        return Err(CommandError::new(
            "invalid_recent_vault",
            "Astian refused an invalid vault identity.",
        ));
    }
    let path = canonical_vault_root.to_str().ok_or_else(|| {
        CommandError::new(
            "unsupported_vault_path",
            "The vault path is not supported on this platform.",
        )
    })?;
    Ok(hash_bytes(path.as_bytes()))
}

fn read_session(
    app_local_data_root: &Path,
) -> Result<(StoredSession, Option<Vec<u8>>), CommandError> {
    let target = app_local_data_root.join(SESSION_FILE_NAME);
    let bytes = match fs::read(&target) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((StoredSession::empty(), None));
        }
        Err(_) => {
            return Err(session_error(
                "session_read_failed",
                "Session state could not be read.",
            ))
        }
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
        session_error(
            "session_corrupt",
            "Session state is invalid and was preserved.",
        )
    })?;
    let schema_version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            session_error(
                "session_corrupt",
                "Session state is invalid and was preserved.",
            )
        })?;
    if schema_version != u64::from(SESSION_SCHEMA_VERSION) {
        return Err(session_error(
            "session_unsupported",
            "Session state uses an unsupported format and was preserved.",
        ));
    }
    let session: StoredSession = serde_json::from_value(value).map_err(|_| {
        session_error(
            "session_corrupt",
            "Session state is invalid and was preserved.",
        )
    })?;
    if session.vaults.len() > MAX_VAULT_SESSIONS
        || session.vaults.iter().any(|entry| {
            entry.vault_id.len() != 64
                || !entry.vault_id.bytes().all(|byte| byte.is_ascii_hexdigit())
                || validate_relative_note_path(&entry.active_note).is_err()
        })
    {
        return Err(session_error(
            "session_corrupt",
            "Session state is invalid and was preserved.",
        ));
    }
    Ok((session, Some(bytes)))
}

fn write_session<F>(
    app_local_data_root: &Path,
    session: &StoredSession,
    expected_bytes: Option<&[u8]>,
    replace_file: F,
) -> Result<(), CommandError>
where
    F: Fn(&Path, &Path) -> io::Result<ReplacementReceipt>,
{
    fs::create_dir_all(app_local_data_root).map_err(|_| {
        session_error(
            "session_prepare_failed",
            "Session storage could not be prepared.",
        )
    })?;
    let mut encoded = serde_json::to_vec_pretty(session).map_err(|_| CommandError::internal())?;
    encoded.push(b'\n');
    let target = app_local_data_root.join(SESSION_FILE_NAME);
    let mut temporary = TempFileBuilder::new()
        .prefix(".astian-session-")
        .suffix(".tmp")
        .tempfile_in(app_local_data_root)
        .map_err(|_| {
            session_error(
                "session_prepare_failed",
                "Session state could not be prepared.",
            )
        })?;
    temporary
        .write_all(&encoded)
        .and_then(|_| temporary.flush())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| {
            session_error(
                "session_write_failed",
                "Session state could not be written safely.",
            )
        })?;
    let (temporary_file, temporary_path) = temporary.keep().map_err(|_| {
        session_error(
            "session_prepare_failed",
            "Session state could not be prepared.",
        )
    })?;
    drop(temporary_file);
    let _temporary_cleanup = CleanupPath(temporary_path.clone());

    let receipt = install_session_file(&target, &temporary_path, expected_bytes, &replace_file)?;
    let verified = fs::read(&target).map_err(|_| {
        session_error(
            "session_verification_failed",
            "Astian could not verify session state.",
        )
    })?;
    if verified != encoded {
        return Err(session_error(
            "session_verification_failed",
            "Astian could not verify session state.",
        ));
    }
    if let Some(recovery_path) = receipt.and_then(|value| value.recovery_path) {
        let _ = fs::remove_file(recovery_path);
    }
    Ok(())
}

fn install_session_file<F>(
    target: &Path,
    temporary_path: &Path,
    expected_bytes: Option<&[u8]>,
    replace_file: &F,
) -> Result<Option<ReplacementReceipt>, CommandError>
where
    F: Fn(&Path, &Path) -> io::Result<ReplacementReceipt>,
{
    let Some(expected_bytes) = expected_bytes else {
        return match fs::hard_link(temporary_path, target) {
            Ok(()) => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(session_error(
                "session_changed",
                "Session state changed in another process and was preserved.",
            )),
            Err(_) => Err(session_error(
                "session_replace_failed",
                "Session state could not be installed safely.",
            )),
        };
    };

    for delay_ms in REPLACE_RETRY_DELAYS_MS {
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms));
        }
        let current = fs::read(target).map_err(|_| {
            session_error(
                "session_changed",
                "Session state changed in another process and was preserved.",
            )
        })?;
        if current != expected_bytes {
            return Err(session_error(
                "session_changed",
                "Session state changed in another process and was preserved.",
            ));
        }
        match replace_file(target, temporary_path) {
            Ok(receipt) => return Ok(Some(receipt)),
            Err(error) if is_retryable_replace_error(&error) => continue,
            Err(_) => {
                return Err(session_error(
                    "session_replace_failed",
                    "Session state could not be replaced safely.",
                ));
            }
        }
    }
    Err(session_error(
        "session_locked",
        "Another process is using session state. The previous state was preserved.",
    ))
}

fn session_error(code: &'static str, message: &'static str) -> CommandError {
    CommandError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn canonical_directory(parent: &Path, name: &str) -> std::path::PathBuf {
        let path = parent.join(name);
        fs::create_dir_all(&path).expect("fixture directory should be created");
        fs::canonicalize(path).expect("fixture should canonicalize")
    }

    fn artifacts(app_data: &Path) -> Vec<std::path::PathBuf> {
        fs::read_dir(app_data)
            .expect("app data should be readable")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".astian-session-"))
            })
            .collect()
    }

    #[test]
    fn active_note_round_trip_is_versioned_and_outside_vault() {
        let temp = tempfile::tempdir().expect("temporary root should be created");
        let app_data = temp.path().join("app-data");
        let vault = canonical_directory(temp.path(), "Kho ghi chú");

        remember_active_note(&app_data, &vault, Some("Dự án/Kế hoạch.md"))
            .expect("active note should persist");

        assert_eq!(
            active_note(&app_data, &vault).expect("active note should read"),
            Some("Dự án/Kế hoạch.md".to_owned())
        );
        assert!(app_data.join(SESSION_FILE_NAME).is_file());
        assert!(!vault.join(SESSION_FILE_NAME).exists());
        assert!(artifacts(&app_data).is_empty());
    }

    #[test]
    fn clearing_one_vault_session_preserves_other_vaults() {
        let temp = tempfile::tempdir().expect("temporary root should be created");
        let app_data = temp.path().join("app-data");
        let first = canonical_directory(temp.path(), "first");
        let second = canonical_directory(temp.path(), "second");
        remember_active_note(&app_data, &first, Some("first.md"))
            .expect("first session should write");
        remember_active_note(&app_data, &second, Some("second.md"))
            .expect("second session should write");

        remember_active_note(&app_data, &first, None).expect("first session should clear");

        assert_eq!(
            active_note(&app_data, &first).expect("first should read"),
            None
        );
        assert_eq!(
            active_note(&app_data, &second).expect("second should read"),
            Some("second.md".to_owned())
        );
        assert!(artifacts(&app_data).is_empty());
    }

    #[test]
    fn traversal_and_corrupt_state_are_preserved() {
        let temp = tempfile::tempdir().expect("temporary root should be created");
        let app_data = temp.path().join("app-data");
        let vault = canonical_directory(temp.path(), "vault");
        let invalid = remember_active_note(&app_data, &vault, Some("../outside.md"))
            .expect_err("traversal should fail");
        assert_eq!(invalid.code, "invalid_note_path");
        assert!(!app_data.join(SESSION_FILE_NAME).exists());

        fs::create_dir_all(&app_data).expect("app data should be created");
        let target = app_data.join(SESSION_FILE_NAME);
        fs::write(&target, b"not-json").expect("corrupt fixture should write");
        let corrupt = active_note(&app_data, &vault).expect_err("corrupt state should fail");
        assert_eq!(corrupt.code, "session_corrupt");
        assert_eq!(
            fs::read(target).expect("corrupt state should remain"),
            b"not-json"
        );
    }

    #[test]
    fn failed_replace_preserves_previous_session_and_cleans_temp() {
        let temp = tempfile::tempdir().expect("temporary root should be created");
        let app_data = temp.path().join("app-data");
        let vault = canonical_directory(temp.path(), "vault");
        remember_active_note(&app_data, &vault, Some("first.md"))
            .expect("initial state should write");
        let target = app_data.join(SESSION_FILE_NAME);
        let original = fs::read(&target).expect("session should read");
        let calls = Cell::new(0);

        let error = remember_active_note_with(&app_data, &vault, Some("second.md"), |_, _| {
            calls.set(calls.get() + 1);
            Err(io::Error::from_raw_os_error(5))
        })
        .expect_err("locked session should fail");

        assert_eq!(error.code, "session_locked");
        assert_eq!(calls.get(), REPLACE_RETRY_DELAYS_MS.len());
        assert_eq!(fs::read(target).expect("session should remain"), original);
        assert!(artifacts(&app_data).is_empty());
    }
}
