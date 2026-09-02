use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};
use tempfile::Builder as TempFileBuilder;

use super::{
    is_retryable_replace_error, platform_replace_file, CleanupPath, CommandError,
    ReplacementReceipt, REPLACE_RETRY_DELAYS_MS,
};

const SETTINGS_SCHEMA_VERSION: u32 = 1;
const SETTINGS_FILE_NAME: &str = "settings.json";
const MAX_RECENT_VAULTS: usize = 10;

#[derive(Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredSettings {
    schema_version: u32,
    recent_vaults: Vec<String>,
}

impl StoredSettings {
    fn empty() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            recent_vaults: Vec::new(),
        }
    }
}

pub(crate) fn last_vault(app_local_data_root: &Path) -> Result<Option<PathBuf>, CommandError> {
    let (settings, _) = read_settings(app_local_data_root)?;
    Ok(settings.recent_vaults.first().map(PathBuf::from))
}

pub(crate) fn remember_vault(
    app_local_data_root: &Path,
    canonical_vault_root: &Path,
) -> Result<(), CommandError> {
    remember_vault_with(
        app_local_data_root,
        canonical_vault_root,
        platform_replace_file,
    )
}

fn remember_vault_with<F>(
    app_local_data_root: &Path,
    canonical_vault_root: &Path,
    replace_file: F,
) -> Result<(), CommandError>
where
    F: Fn(&Path, &Path) -> io::Result<ReplacementReceipt>,
{
    if !canonical_vault_root.is_absolute() || !canonical_vault_root.is_dir() {
        return Err(CommandError::new(
            "invalid_recent_vault",
            "Astian refused to remember an invalid vault location.",
        ));
    }
    let vault_path = canonical_vault_root.to_str().ok_or_else(|| {
        CommandError::new(
            "unsupported_vault_path",
            "The selected vault path is not supported on this platform.",
        )
    })?;

    let (mut settings, expected_bytes) = read_settings(app_local_data_root)?;
    if settings
        .recent_vaults
        .first()
        .is_some_and(|path| path == vault_path)
    {
        return Ok(());
    }
    settings.recent_vaults.retain(|path| path != vault_path);
    settings.recent_vaults.insert(0, vault_path.to_owned());
    settings.recent_vaults.truncate(MAX_RECENT_VAULTS);

    write_settings(
        app_local_data_root,
        &settings,
        expected_bytes.as_deref(),
        replace_file,
    )
}

fn read_settings(
    app_local_data_root: &Path,
) -> Result<(StoredSettings, Option<Vec<u8>>), CommandError> {
    let target = app_local_data_root.join(SETTINGS_FILE_NAME);
    let bytes = match fs::read(&target) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((StoredSettings::empty(), None));
        }
        Err(_) => {
            return Err(CommandError::new(
                "settings_read_failed",
                "Astian settings could not be read.",
            ));
        }
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
        CommandError::new(
            "settings_corrupt",
            "Astian settings are invalid and were left unchanged.",
        )
    })?;
    let schema_version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            CommandError::new(
                "settings_corrupt",
                "Astian settings are invalid and were left unchanged.",
            )
        })?;
    if schema_version != u64::from(SETTINGS_SCHEMA_VERSION) {
        return Err(CommandError::new(
            "settings_unsupported",
            "Astian settings use an unsupported format and were left unchanged.",
        ));
    }

    let settings: StoredSettings = serde_json::from_value(value).map_err(|_| {
        CommandError::new(
            "settings_corrupt",
            "Astian settings are invalid and were left unchanged.",
        )
    })?;
    if settings.recent_vaults.len() > MAX_RECENT_VAULTS
        || settings
            .recent_vaults
            .iter()
            .any(|path| path.is_empty() || !Path::new(path).is_absolute())
    {
        return Err(CommandError::new(
            "settings_corrupt",
            "Astian settings are invalid and were left unchanged.",
        ));
    }

    Ok((settings, Some(bytes)))
}

fn write_settings<F>(
    app_local_data_root: &Path,
    settings: &StoredSettings,
    expected_bytes: Option<&[u8]>,
    replace_file: F,
) -> Result<(), CommandError>
where
    F: Fn(&Path, &Path) -> io::Result<ReplacementReceipt>,
{
    fs::create_dir_all(app_local_data_root).map_err(|_| {
        CommandError::new(
            "settings_prepare_failed",
            "The Astian settings directory could not be prepared.",
        )
    })?;
    let mut encoded = serde_json::to_vec_pretty(settings).map_err(|_| CommandError::internal())?;
    encoded.push(b'\n');
    let target = app_local_data_root.join(SETTINGS_FILE_NAME);
    let mut temporary = TempFileBuilder::new()
        .prefix(".astian-settings-")
        .suffix(".tmp")
        .tempfile_in(app_local_data_root)
        .map_err(|_| {
            CommandError::new(
                "settings_prepare_failed",
                "Astian settings could not be prepared.",
            )
        })?;
    temporary
        .write_all(&encoded)
        .and_then(|_| temporary.flush())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| {
            CommandError::new(
                "settings_write_failed",
                "Astian settings could not be written safely.",
            )
        })?;
    let (temporary_file, temporary_path) = temporary.keep().map_err(|_| {
        CommandError::new(
            "settings_prepare_failed",
            "Astian settings could not be prepared.",
        )
    })?;
    drop(temporary_file);
    let _temporary_cleanup = CleanupPath(temporary_path.clone());

    let receipt = install_settings_file(&target, &temporary_path, expected_bytes, &replace_file)?;
    let verified = fs::read(&target).map_err(|_| {
        CommandError::new(
            "settings_verification_failed",
            "Astian could not verify the saved settings.",
        )
    })?;
    if verified != encoded {
        return Err(CommandError::new(
            "settings_verification_failed",
            "Astian could not verify the saved settings.",
        ));
    }
    if let Some(recovery_path) = receipt.and_then(|value| value.recovery_path) {
        let _ = fs::remove_file(recovery_path);
    }

    Ok(())
}

fn install_settings_file<F>(
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
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(CommandError::new(
                "settings_changed",
                "Astian settings changed in another process and were left unchanged.",
            )),
            Err(_) => Err(CommandError::new(
                "settings_replace_failed",
                "Astian settings could not be installed safely.",
            )),
        };
    };

    for delay_ms in REPLACE_RETRY_DELAYS_MS {
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms));
        }
        let current = fs::read(target).map_err(|_| {
            CommandError::new(
                "settings_changed",
                "Astian settings changed in another process and were left unchanged.",
            )
        })?;
        if current != expected_bytes {
            return Err(CommandError::new(
                "settings_changed",
                "Astian settings changed in another process and were left unchanged.",
            ));
        }
        match replace_file(target, temporary_path) {
            Ok(receipt) => return Ok(Some(receipt)),
            Err(error) if is_retryable_replace_error(&error) => continue,
            Err(_) => {
                return Err(CommandError::new(
                    "settings_replace_failed",
                    "Astian settings could not be replaced safely.",
                ));
            }
        }
    }

    Err(CommandError::new(
        "settings_locked",
        "Another process is using Astian settings. The previous settings were preserved.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn canonical_directory(parent: &Path, name: &str) -> PathBuf {
        let path = parent.join(name);
        fs::create_dir_all(&path).expect("fixture directory should be created");
        fs::canonicalize(path).expect("fixture directory should canonicalize")
    }

    fn internal_artifacts(root: &Path) -> Vec<PathBuf> {
        fs::read_dir(root)
            .expect("settings directory should be readable")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".astian-settings-"))
            })
            .collect()
    }

    #[test]
    fn recent_vault_round_trip_preserves_unicode_and_stays_outside_vault() {
        let temp = tempfile::tempdir().expect("temporary root should be created");
        let app_data = temp.path().join("app-data");
        let vault = canonical_directory(temp.path(), "Kho ghi chú");

        remember_vault(&app_data, &vault).expect("vault should be remembered");

        assert_eq!(
            last_vault(&app_data).expect("last vault should read"),
            Some(vault.clone())
        );
        assert!(app_data.join(SETTINGS_FILE_NAME).is_file());
        assert!(!vault.join(SETTINGS_FILE_NAME).exists());
        assert!(internal_artifacts(&app_data).is_empty());
    }

    #[test]
    fn recent_vaults_are_deduplicated_ordered_and_bounded() {
        let temp = tempfile::tempdir().expect("temporary root should be created");
        let app_data = temp.path().join("app-data");
        let mut vaults = Vec::new();
        for index in 0..12 {
            let vault = canonical_directory(temp.path(), &format!("vault-{index}"));
            remember_vault(&app_data, &vault).expect("vault should be remembered");
            vaults.push(vault);
        }
        remember_vault(&app_data, &vaults[5]).expect("existing vault should move to front");

        let (settings, _) = read_settings(&app_data).expect("settings should read");
        assert_eq!(settings.recent_vaults.len(), MAX_RECENT_VAULTS);
        assert_eq!(settings.recent_vaults[0], vaults[5].to_string_lossy());
        assert_eq!(
            settings
                .recent_vaults
                .iter()
                .filter(|path| *path == &vaults[5].to_string_lossy())
                .count(),
            1
        );
    }

    #[test]
    fn corrupt_and_unsupported_settings_are_preserved() {
        let temp = tempfile::tempdir().expect("temporary root should be created");
        let app_data = temp.path().join("app-data");
        fs::create_dir_all(&app_data).expect("app data should be created");
        let target = app_data.join(SETTINGS_FILE_NAME);
        fs::write(&target, b"not-json").expect("corrupt fixture should write");

        let corrupt = last_vault(&app_data).expect_err("corrupt settings must fail closed");
        assert_eq!(corrupt.code, "settings_corrupt");
        assert_eq!(
            fs::read(&target).expect("fixture should remain"),
            b"not-json"
        );

        let unsupported = br#"{"schema_version":2,"recent_vaults":[]}"#;
        fs::write(&target, unsupported).expect("unsupported fixture should write");
        let error = last_vault(&app_data).expect_err("unsupported settings must fail closed");
        assert_eq!(error.code, "settings_unsupported");
        assert_eq!(
            fs::read(&target).expect("unsupported fixture should remain"),
            unsupported
        );
    }

    #[test]
    fn failed_replace_preserves_previous_settings_and_cleans_temp() {
        let temp = tempfile::tempdir().expect("temporary root should be created");
        let app_data = temp.path().join("app-data");
        let first = canonical_directory(temp.path(), "first");
        let second = canonical_directory(temp.path(), "second");
        remember_vault(&app_data, &first).expect("initial settings should write");
        let original = fs::read(app_data.join(SETTINGS_FILE_NAME)).expect("settings should read");
        let calls = Cell::new(0);

        let error = remember_vault_with(&app_data, &second, |_, _| {
            calls.set(calls.get() + 1);
            Err(io::Error::from_raw_os_error(5))
        })
        .expect_err("locked replacement should fail");

        assert_eq!(error.code, "settings_locked");
        assert_eq!(calls.get(), REPLACE_RETRY_DELAYS_MS.len());
        assert_eq!(
            fs::read(app_data.join(SETTINGS_FILE_NAME)).expect("settings should remain"),
            original
        );
        assert!(internal_artifacts(&app_data).is_empty());
    }

    #[test]
    fn replacement_race_preserves_external_settings_revision() {
        let temp = tempfile::tempdir().expect("temporary root should be created");
        let app_data = temp.path().join("app-data");
        let first = canonical_directory(temp.path(), "first");
        let second = canonical_directory(temp.path(), "second");
        remember_vault(&app_data, &first).expect("initial settings should write");
        let target = app_data.join(SETTINGS_FILE_NAME);

        let error = remember_vault_with(&app_data, &second, |target, _| {
            fs::write(target, b"external revision")?;
            Err(io::Error::from_raw_os_error(5))
        })
        .expect_err("changed settings should not be overwritten");

        assert_eq!(error.code, "settings_changed");
        assert_eq!(
            fs::read(target).expect("external revision should remain"),
            b"external revision"
        );
        assert!(internal_artifacts(&app_data).is_empty());
    }
}
