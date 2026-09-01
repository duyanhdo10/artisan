mod recovery;
pub mod search_index;
mod watcher;

use recovery::{RecoveryDraft, RecoveryDraftListItem, RecoveryDraftSummary};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use tempfile::Builder as TempFileBuilder;

#[cfg(windows)]
use std::{os::windows::ffi::OsStrExt, ptr};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";
const REPLACE_RETRY_DELAYS_MS: [u64; 3] = [0, 25, 75];

/// Stable error envelope returned across the Tauri IPC boundary.
///
/// Frontend logic must branch on `code`, never on the human-readable message.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    code: &'static str,
    message: String,
}

impl CommandError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn internal() -> Self {
        Self::new("internal_error", "Astian could not complete the operation.")
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    app_version: &'static str,
    platform: &'static str,
    architecture: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteEntry {
    relative_path: String,
    title: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultSummary {
    name: String,
    notes: Vec<NoteEntry>,
    vault_session: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedNote {
    relative_path: String,
    content: String,
    content_hash: String,
    line_ending: LineEnding,
    has_utf8_bom: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEnding {
    Lf,
    #[serde(rename = "crlf")]
    CrLf,
    Mixed,
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveStatus {
    Saved,
    Unchanged,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    status: SaveStatus,
    content_hash: String,
}

#[derive(Default)]
struct VaultState {
    root: Mutex<Option<PathBuf>>,
    write_lock: Arc<Mutex<()>>,
    watcher: Mutex<Option<watcher::VaultWatcher>>,
    expected_writes: Mutex<watcher::ExpectedWrites>,
    next_vault_session: AtomicU64,
}

#[tauri::command]
fn get_runtime_info() -> Result<RuntimeInfo, CommandError> {
    Ok(RuntimeInfo {
        app_version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
    })
}

#[tauri::command]
async fn select_vault(
    app: tauri::AppHandle,
    state: tauri::State<'_, VaultState>,
) -> Result<Option<VaultSummary>, CommandError> {
    let Some(selected) = app.dialog().file().blocking_pick_folder() else {
        return Ok(None);
    };

    let selected_path = selected.into_path().map_err(|_| {
        CommandError::new(
            "unsupported_vault_path",
            "The selected vault path is not supported on this platform.",
        )
    })?;
    let root = fs::canonicalize(selected_path).map_err(|_| {
        CommandError::new("vault_unavailable", "The selected vault is unavailable.")
    })?;

    if !root.is_dir() {
        return Err(CommandError::new(
            "vault_not_directory",
            "A vault must be a folder.",
        ));
    }

    let notes = list_markdown_notes(&root)?;
    let name = root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("Vault")
        .to_owned();

    let vault_session = state
        .next_vault_session
        .fetch_add(1, Ordering::Relaxed)
        .saturating_add(1);
    let expected_writes = Arc::new(Mutex::new(HashMap::new()));
    let next_watcher = watcher::start_vault_watcher(
        app,
        root.clone(),
        vault_session,
        Arc::clone(&expected_writes),
        Arc::clone(&state.write_lock),
    )?;

    *state.watcher.lock().map_err(|_| CommandError::internal())? = Some(next_watcher);
    *state
        .expected_writes
        .lock()
        .map_err(|_| CommandError::internal())? = expected_writes;
    *state.root.lock().map_err(|_| CommandError::internal())? = Some(root);

    Ok(Some(VaultSummary {
        name,
        notes,
        vault_session,
    }))
}

#[tauri::command]
fn open_note(
    relative_path: String,
    state: tauri::State<'_, VaultState>,
) -> Result<OpenedNote, CommandError> {
    let root = state
        .root
        .lock()
        .map_err(|_| CommandError::internal())?
        .clone()
        .ok_or_else(|| CommandError::new("vault_not_open", "Open a vault first."))?;

    read_markdown_note(&root, &relative_path)
}

#[tauri::command]
async fn save_note(
    app: tauri::AppHandle,
    relative_path: String,
    content: String,
    expected_hash: String,
    state: tauri::State<'_, VaultState>,
) -> Result<SaveResult, CommandError> {
    let root = state
        .root
        .lock()
        .map_err(|_| CommandError::internal())?
        .clone()
        .ok_or_else(|| CommandError::new("vault_not_open", "Open a vault first."))?;
    let app_local_data_root = app_local_data_root(&app)?;
    let write_lock = Arc::clone(&state.write_lock);
    let expected_writes = current_expected_writes(&state)?;

    tauri::async_runtime::spawn_blocking(move || {
        let _guard = write_lock.lock().map_err(|_| CommandError::internal())?;
        let result = save_markdown_note_and_cleanup(
            &root,
            &relative_path,
            &content,
            &expected_hash,
            || {
                recovery::clear_recovery_draft_if_content_matches(
                    &app_local_data_root,
                    &root,
                    &relative_path,
                    &content,
                )
            },
        )?;
        if result.status == SaveStatus::Saved {
            watcher::record_expected_write(&expected_writes, &relative_path, &result.content_hash)?;
        }
        Ok(result)
    })
    .await
    .map_err(|_| CommandError::internal())?
}

#[tauri::command]
async fn write_recovery_draft(
    app: tauri::AppHandle,
    relative_path: String,
    content: String,
    base_hash: String,
    editor_revision: u64,
    expected_draft_hash: Option<String>,
    state: tauri::State<'_, VaultState>,
) -> Result<RecoveryDraftSummary, CommandError> {
    let root = current_vault_root(&state)?;
    let app_local_data_root = app_local_data_root(&app)?;
    let write_lock = Arc::clone(&state.write_lock);

    tauri::async_runtime::spawn_blocking(move || {
        let _guard = write_lock.lock().map_err(|_| CommandError::internal())?;
        recovery::write_recovery_draft(
            &app_local_data_root,
            &root,
            &relative_path,
            &content,
            &base_hash,
            editor_revision,
            expected_draft_hash.as_deref(),
        )
    })
    .await
    .map_err(|_| CommandError::internal())?
}

#[tauri::command]
async fn list_recovery_drafts(
    app: tauri::AppHandle,
    state: tauri::State<'_, VaultState>,
) -> Result<Vec<RecoveryDraftListItem>, CommandError> {
    let root = current_vault_root(&state)?;
    let app_local_data_root = app_local_data_root(&app)?;
    let write_lock = Arc::clone(&state.write_lock);

    tauri::async_runtime::spawn_blocking(move || {
        let _guard = write_lock.lock().map_err(|_| CommandError::internal())?;
        recovery::list_recovery_drafts(&app_local_data_root, &root)
    })
    .await
    .map_err(|_| CommandError::internal())?
}

#[tauri::command]
async fn read_recovery_draft(
    app: tauri::AppHandle,
    relative_path: String,
    state: tauri::State<'_, VaultState>,
) -> Result<RecoveryDraft, CommandError> {
    let root = current_vault_root(&state)?;
    let app_local_data_root = app_local_data_root(&app)?;
    let write_lock = Arc::clone(&state.write_lock);

    tauri::async_runtime::spawn_blocking(move || {
        let _guard = write_lock.lock().map_err(|_| CommandError::internal())?;
        recovery::read_recovery_draft(&app_local_data_root, &root, &relative_path)
    })
    .await
    .map_err(|_| CommandError::internal())?
}

#[tauri::command]
async fn clear_recovery_draft(
    app: tauri::AppHandle,
    relative_path: String,
    state: tauri::State<'_, VaultState>,
) -> Result<(), CommandError> {
    let root = current_vault_root(&state)?;
    let app_local_data_root = app_local_data_root(&app)?;
    let write_lock = Arc::clone(&state.write_lock);

    tauri::async_runtime::spawn_blocking(move || {
        let _guard = write_lock.lock().map_err(|_| CommandError::internal())?;
        recovery::clear_recovery_draft(&app_local_data_root, &root, &relative_path)
    })
    .await
    .map_err(|_| CommandError::internal())?
}

#[tauri::command]
async fn export_unavailable_recovery_draft(
    app: tauri::AppHandle,
    recovery_id: String,
    expected_artifact_hash: String,
    state: tauri::State<'_, VaultState>,
) -> Result<bool, CommandError> {
    let root = current_vault_root(&state)?;
    let suggested_name = recovery::suggested_unavailable_export_name(&recovery_id)?;
    let Some(selected) = app
        .dialog()
        .file()
        .set_file_name(suggested_name)
        .add_filter("Recovery data", &["json"])
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let selected_path = selected.into_path().map_err(|_| {
        CommandError::new(
            "unsupported_recovery_export_path",
            "The selected recovery export path is not supported on this platform.",
        )
    })?;
    let app_local_data_root = app_local_data_root(&app)?;
    let write_lock = Arc::clone(&state.write_lock);

    tauri::async_runtime::spawn_blocking(move || {
        let _guard = write_lock.lock().map_err(|_| CommandError::internal())?;
        recovery::export_unavailable_recovery_draft(
            &app_local_data_root,
            &root,
            &recovery_id,
            &expected_artifact_hash,
            &selected_path,
        )?;
        Ok(true)
    })
    .await
    .map_err(|_| CommandError::internal())?
}

#[tauri::command]
async fn delete_unavailable_recovery_draft(
    app: tauri::AppHandle,
    recovery_id: String,
    expected_artifact_hash: String,
    state: tauri::State<'_, VaultState>,
) -> Result<(), CommandError> {
    let root = current_vault_root(&state)?;
    let app_local_data_root = app_local_data_root(&app)?;
    let write_lock = Arc::clone(&state.write_lock);

    tauri::async_runtime::spawn_blocking(move || {
        let _guard = write_lock.lock().map_err(|_| CommandError::internal())?;
        recovery::delete_unavailable_recovery_draft(
            &app_local_data_root,
            &root,
            &recovery_id,
            &expected_artifact_hash,
        )
    })
    .await
    .map_err(|_| CommandError::internal())?
}

#[tauri::command]
async fn save_note_as_copy(
    app: tauri::AppHandle,
    source_relative_path: String,
    content: String,
    state: tauri::State<'_, VaultState>,
) -> Result<Option<OpenedNote>, CommandError> {
    let root = current_vault_root(&state)?;
    let suggested_name = suggested_recovery_copy_name(&source_relative_path)?;
    let Some(selected) = app
        .dialog()
        .file()
        .set_directory(&root)
        .set_file_name(suggested_name)
        .add_filter("Markdown", &["md"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let selected_path = selected.into_path().map_err(|_| {
        CommandError::new(
            "unsupported_copy_path",
            "The selected copy path is not supported on this platform.",
        )
    })?;
    let app_local_data_root = app_local_data_root(&app)?;
    let write_lock = Arc::clone(&state.write_lock);
    let expected_writes = current_expected_writes(&state)?;

    tauri::async_runtime::spawn_blocking(move || {
        let _guard = write_lock.lock().map_err(|_| CommandError::internal())?;
        let copied = create_markdown_copy_and_cleanup(&root, &selected_path, &content, || {
            recovery::clear_recovery_draft_if_content_matches(
                &app_local_data_root,
                &root,
                &source_relative_path,
                &content,
            )
        })?;
        watcher::record_expected_write(
            &expected_writes,
            &copied.relative_path,
            &copied.content_hash,
        )?;
        Ok(Some(copied))
    })
    .await
    .map_err(|_| CommandError::internal())?
}

#[tauri::command]
fn reconcile_vault(state: tauri::State<'_, VaultState>) -> Result<(), CommandError> {
    state
        .watcher
        .lock()
        .map_err(|_| CommandError::internal())?
        .as_ref()
        .ok_or_else(|| CommandError::new("vault_not_open", "Open a vault first."))?
        .request_reconcile()
}

fn current_vault_root(state: &VaultState) -> Result<PathBuf, CommandError> {
    state
        .root
        .lock()
        .map_err(|_| CommandError::internal())?
        .clone()
        .ok_or_else(|| CommandError::new("vault_not_open", "Open a vault first."))
}

fn current_expected_writes(state: &VaultState) -> Result<watcher::ExpectedWrites, CommandError> {
    state
        .expected_writes
        .lock()
        .map_err(|_| CommandError::internal())
        .map(|expected| Arc::clone(&expected))
}

fn app_local_data_root(app: &tauri::AppHandle) -> Result<PathBuf, CommandError> {
    app.path().app_local_data_dir().map_err(|_| {
        CommandError::new(
            "recovery_location_unavailable",
            "The recovery draft location is unavailable.",
        )
    })
}

fn validate_relative_note_path(relative_path: &str) -> Result<PathBuf, CommandError> {
    let path = Path::new(relative_path);
    let is_safe_relative = !relative_path.is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    let is_markdown = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));

    if !is_safe_relative || !is_markdown {
        return Err(CommandError::new(
            "invalid_note_path",
            "The note path must be a relative Markdown path inside the vault.",
        ));
    }

    Ok(path.to_path_buf())
}

fn suggested_recovery_copy_name(relative_path: &str) -> Result<String, CommandError> {
    let relative = validate_relative_note_path(relative_path)?;
    let stem = relative
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            CommandError::new(
                "unsupported_copy_path",
                "The recovery note name is not supported on this platform.",
            )
        })?;
    Ok(format!("{stem}-recovered.md"))
}

fn create_markdown_copy(
    root: &Path,
    selected_path: &Path,
    content: &str,
) -> Result<OpenedNote, CommandError> {
    create_markdown_copy_with(root, selected_path, content, |source, target| {
        fs::hard_link(source, target)
    })
}

fn create_markdown_copy_and_cleanup<F>(
    root: &Path,
    selected_path: &Path,
    content: &str,
    cleanup_recovery: F,
) -> Result<OpenedNote, CommandError>
where
    F: FnOnce() -> Result<(), CommandError>,
{
    let copied = create_markdown_copy(root, selected_path, content)?;
    let _ = cleanup_recovery();
    Ok(copied)
}

fn create_markdown_copy_with<F>(
    root: &Path,
    selected_path: &Path,
    content: &str,
    install_copy: F,
) -> Result<OpenedNote, CommandError>
where
    F: Fn(&Path, &Path) -> io::Result<()>,
{
    let file_name = selected_path.file_name().ok_or_else(|| {
        CommandError::new(
            "invalid_copy_path",
            "Choose a Markdown filename inside the current vault.",
        )
    })?;
    let is_markdown = selected_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
    if !is_markdown {
        return Err(CommandError::new(
            "invalid_copy_path",
            "The recovery copy must use a Markdown filename.",
        ));
    }

    let parent = selected_path.parent().ok_or_else(|| {
        CommandError::new(
            "invalid_copy_path",
            "Choose a Markdown filename inside the current vault.",
        )
    })?;
    let canonical_parent = fs::canonicalize(parent).map_err(|_| {
        CommandError::new(
            "copy_parent_unavailable",
            "The selected recovery copy folder is unavailable.",
        )
    })?;
    if !canonical_parent.starts_with(root) {
        return Err(CommandError::new(
            "copy_outside_vault",
            "The recovery copy must stay inside the current vault.",
        ));
    }

    let target = canonical_parent.join(file_name);
    if target.try_exists().map_err(|_| {
        CommandError::new(
            "copy_target_unavailable",
            "The recovery copy destination could not be inspected.",
        )
    })? {
        return Err(CommandError::new(
            "copy_already_exists",
            "Astian will not overwrite an existing file. Choose a new name.",
        ));
    }

    let relative = target.strip_prefix(root).map_err(|_| {
        CommandError::new(
            "copy_outside_vault",
            "The recovery copy must stay inside the current vault.",
        )
    })?;
    let relative_path = relative
        .to_str()
        .ok_or_else(|| {
            CommandError::new(
                "unsupported_copy_path",
                "The selected copy path cannot be represented safely.",
            )
        })?
        .replace('\\', "/");
    validate_relative_note_path(&relative_path)?;

    let normalized_content = content.replace("\r\n", "\n").replace('\r', "\n");
    let expected_bytes = normalized_content.as_bytes();
    let mut temporary = TempFileBuilder::new()
        .prefix(".astian-copy-")
        .suffix(".tmp")
        .tempfile_in(&canonical_parent)
        .map_err(|_| {
            CommandError::new(
                "copy_prepare_failed",
                "The recovery copy could not be prepared.",
            )
        })?;
    temporary
        .write_all(expected_bytes)
        .and_then(|_| temporary.flush())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| {
            CommandError::new(
                "copy_write_failed",
                "The recovery copy could not be written safely.",
            )
        })?;
    let (temporary_file, temporary_path) = temporary.keep().map_err(|_| {
        CommandError::new(
            "copy_prepare_failed",
            "The recovery copy could not be prepared.",
        )
    })?;
    drop(temporary_file);
    let _temporary_cleanup = CleanupPath(temporary_path.clone());

    install_copy(&temporary_path, &target).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            CommandError::new(
                "copy_already_exists",
                "Astian will not overwrite an existing file. Choose a new name.",
            )
        } else {
            CommandError::new(
                "copy_install_failed",
                "The recovery copy could not be installed safely.",
            )
        }
    })?;

    let copied_bytes = fs::read(&target).map_err(|_| {
        CommandError::new(
            "copy_verification_failed",
            "Astian could not verify the recovery copy.",
        )
    })?;
    if copied_bytes != expected_bytes {
        return Err(CommandError::new(
            "copy_verification_failed",
            "Astian could not verify the recovery copy.",
        ));
    }

    read_markdown_note(root, &relative_path)
}

fn read_markdown_note(root: &Path, relative_path: &str) -> Result<OpenedNote, CommandError> {
    let relative = validate_relative_note_path(relative_path)?;
    let canonical_path = fs::canonicalize(root.join(relative))
        .map_err(|_| CommandError::new("note_unavailable", "The selected note is unavailable."))?;

    if !canonical_path.starts_with(root) || !canonical_path.is_file() {
        return Err(CommandError::new(
            "path_outside_vault",
            "Astian refused to access a path outside the current vault.",
        ));
    }

    let bytes = fs::read(canonical_path)
        .map_err(|_| CommandError::new("note_read_failed", "The note could not be read."))?;
    let content_hash = hash_bytes(&bytes);
    let decoded = decode_markdown(&bytes)?;

    Ok(OpenedNote {
        relative_path: relative_path.to_owned(),
        content: decoded.content,
        content_hash,
        line_ending: decoded.line_ending,
        has_utf8_bom: decoded.has_utf8_bom,
    })
}

struct DecodedMarkdown {
    content: String,
    line_ending: LineEnding,
    has_utf8_bom: bool,
}

struct ReplacementReceipt {
    recovery_path: Option<PathBuf>,
}

struct CleanupPath(PathBuf);

impl Drop for CleanupPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_content_hash(content_hash: &str) -> Result<(), CommandError> {
    if content_hash.len() != 64 || !content_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CommandError::new(
            "invalid_content_hash",
            "The request did not contain a valid content hash.",
        ));
    }
    Ok(())
}

fn decode_markdown(bytes: &[u8]) -> Result<DecodedMarkdown, CommandError> {
    let has_utf8_bom = bytes.starts_with(UTF8_BOM);
    let body = if has_utf8_bom {
        &bytes[UTF8_BOM.len()..]
    } else {
        bytes
    };
    let source = String::from_utf8(body.to_vec()).map_err(|_| {
        CommandError::new(
            "unsupported_note_encoding",
            "The note is not valid UTF-8 and was left unchanged.",
        )
    })?;

    let mut crlf_count = 0;
    let mut lf_count = 0;
    let mut lone_cr_count = 0;
    let source_bytes = source.as_bytes();
    let mut index = 0;
    while index < source_bytes.len() {
        match source_bytes[index] {
            b'\r' if source_bytes.get(index + 1) == Some(&b'\n') => {
                crlf_count += 1;
                index += 2;
            }
            b'\r' => {
                lone_cr_count += 1;
                index += 1;
            }
            b'\n' => {
                lf_count += 1;
                index += 1;
            }
            _ => index += 1,
        }
    }

    let line_ending = match (crlf_count, lf_count, lone_cr_count) {
        (0, 0, 0) => LineEnding::None,
        (0, _, 0) => LineEnding::Lf,
        (_, 0, 0) => LineEnding::CrLf,
        _ => LineEnding::Mixed,
    };
    let content = source.replace("\r\n", "\n").replace('\r', "\n");

    Ok(DecodedMarkdown {
        content,
        line_ending,
        has_utf8_bom,
    })
}

fn encode_markdown(content: &str, source: &DecodedMarkdown) -> Result<Vec<u8>, CommandError> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let body = match source.line_ending {
        LineEnding::CrLf => normalized.replace('\n', "\r\n"),
        LineEnding::Lf | LineEnding::None => normalized,
        LineEnding::Mixed => {
            return Err(CommandError::new(
                "mixed_line_endings_read_only",
                "This note uses mixed line endings and remains read-only in this preview.",
            ));
        }
    };

    let mut bytes = Vec::with_capacity(body.len() + usize::from(source.has_utf8_bom) * 3);
    if source.has_utf8_bom {
        bytes.extend_from_slice(UTF8_BOM);
    }
    bytes.extend_from_slice(body.as_bytes());
    Ok(bytes)
}

fn save_markdown_note(
    root: &Path,
    relative_path: &str,
    content: &str,
    expected_hash: &str,
) -> Result<SaveResult, CommandError> {
    save_markdown_note_with(
        root,
        relative_path,
        content,
        expected_hash,
        platform_replace_file,
    )
}

fn save_markdown_note_and_cleanup<F>(
    root: &Path,
    relative_path: &str,
    content: &str,
    expected_hash: &str,
    cleanup_recovery: F,
) -> Result<SaveResult, CommandError>
where
    F: FnOnce() -> Result<(), CommandError>,
{
    let result = save_markdown_note(root, relative_path, content, expected_hash)?;
    let _ = cleanup_recovery();
    Ok(result)
}

fn save_markdown_note_with<F>(
    root: &Path,
    relative_path: &str,
    content: &str,
    expected_hash: &str,
    replace_file: F,
) -> Result<SaveResult, CommandError>
where
    F: Fn(&Path, &Path) -> io::Result<ReplacementReceipt>,
{
    validate_content_hash(expected_hash)?;

    let relative = validate_relative_note_path(relative_path)?;
    let target = fs::canonicalize(root.join(relative))
        .map_err(|_| CommandError::new("note_unavailable", "The selected note is unavailable."))?;
    if !target.starts_with(root) || !target.is_file() {
        return Err(CommandError::new(
            "path_outside_vault",
            "Astian refused to access a path outside the current vault.",
        ));
    }

    let current_bytes = fs::read(&target)
        .map_err(|_| CommandError::new("note_read_failed", "The note could not be read."))?;
    let current_hash = hash_bytes(&current_bytes);
    if !current_hash.eq_ignore_ascii_case(expected_hash) {
        return Err(CommandError::new(
            "external_change_conflict",
            "The note changed outside Astian. Reload it before saving.",
        ));
    }

    let decoded = decode_markdown(&current_bytes)?;
    if content == decoded.content {
        return Ok(SaveResult {
            status: SaveStatus::Unchanged,
            content_hash: current_hash,
        });
    }

    let replacement_bytes = encode_markdown(content, &decoded)?;
    if replacement_bytes == current_bytes {
        return Ok(SaveResult {
            status: SaveStatus::Unchanged,
            content_hash: current_hash,
        });
    }

    let parent = target.parent().ok_or_else(CommandError::internal)?;
    let mut temporary = TempFileBuilder::new()
        .prefix(".astian-write-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|_| {
            CommandError::new(
                "save_prepare_failed",
                "The note could not be prepared for saving.",
            )
        })?;
    temporary
        .write_all(&replacement_bytes)
        .and_then(|_| temporary.flush())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| {
            CommandError::new("save_write_failed", "The note could not be written safely.")
        })?;
    let (temporary_file, temporary_path) = temporary.keep().map_err(|_| {
        CommandError::new(
            "save_prepare_failed",
            "The note could not be prepared for saving.",
        )
    })?;
    drop(temporary_file);
    let _temporary_cleanup = CleanupPath(temporary_path.clone());

    let expected_replacement_hash = hash_bytes(&replacement_bytes);
    let mut last_replace_error = None;
    for delay_ms in REPLACE_RETRY_DELAYS_MS {
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms));
        }

        let latest_bytes = fs::read(&target).map_err(|_| {
            CommandError::new(
                "external_change_conflict",
                "The note changed outside Astian. Reload it before saving.",
            )
        })?;
        if !hash_bytes(&latest_bytes).eq_ignore_ascii_case(expected_hash) {
            return Err(CommandError::new(
                "external_change_conflict",
                "The note changed outside Astian. Reload it before saving.",
            ));
        }

        match replace_file(&target, &temporary_path) {
            Ok(receipt) => {
                let saved_bytes = fs::read(&target).map_err(|_| {
                    CommandError::new(
                        "save_verification_failed",
                        "Astian could not verify the saved note.",
                    )
                })?;
                let saved_hash = hash_bytes(&saved_bytes);
                if saved_hash != expected_replacement_hash {
                    return Err(CommandError::new(
                        "save_verification_failed",
                        "Astian could not verify the saved note.",
                    ));
                }

                if let Some(recovery_path) = receipt.recovery_path {
                    let _ = fs::remove_file(recovery_path);
                }
                return Ok(SaveResult {
                    status: SaveStatus::Saved,
                    content_hash: saved_hash,
                });
            }
            Err(error) if is_retryable_replace_error(&error) => {
                last_replace_error = Some(error);
            }
            Err(_) => {
                return Err(CommandError::new(
                    "save_replace_failed",
                    "The note could not be replaced safely and was left unchanged.",
                ));
            }
        }
    }

    let code = if last_replace_error
        .as_ref()
        .is_some_and(is_retryable_replace_error)
    {
        "save_locked"
    } else {
        "save_replace_failed"
    };
    Err(CommandError::new(
        code,
        "Another program is using this note. Astian left it unchanged.",
    ))
}

fn is_retryable_replace_error(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(windows)]
fn platform_replace_file(target: &Path, replacement: &Path) -> io::Result<ReplacementReceipt> {
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent"))?;
    let backup_placeholder = TempFileBuilder::new()
        .prefix(".astian-backup-")
        .suffix(".bak")
        .tempfile_in(parent)?;
    let backup_path = backup_placeholder.path().to_path_buf();
    drop(backup_placeholder);

    let target_wide: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let replacement_wide: Vec<u16> = replacement
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let backup_wide: Vec<u16> = backup_path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();

    // SAFETY: all pointers reference live, NUL-terminated UTF-16 buffers for the duration
    // of this call. The two reserved pointer arguments are required to be null.
    let replaced = unsafe {
        ReplaceFileW(
            target_wide.as_ptr(),
            replacement_wide.as_ptr(),
            backup_wide.as_ptr(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };

    if replaced != 0 {
        return Ok(ReplacementReceipt {
            recovery_path: backup_path
                .try_exists()
                .unwrap_or(false)
                .then_some(backup_path),
        });
    }

    let replace_error = io::Error::last_os_error();
    recover_backup_after_replace_error(target, &backup_path)?;
    Err(replace_error)
}

fn recover_backup_after_replace_error(target: &Path, backup_path: &Path) -> io::Result<()> {
    let target_exists = target.try_exists()?;
    let backup_exists = backup_path.try_exists()?;

    if !target_exists && backup_exists {
        // A hard link is a no-clobber restore: if another process recreates the target
        // after our existence check, this fails instead of overwriting that new file.
        fs::hard_link(backup_path, target)?;
        fs::remove_file(backup_path)?;
    } else if target_exists && backup_exists {
        let target_matches_backup = fs::read(target)
            .and_then(|target_bytes| fs::read(backup_path).map(|backup| target_bytes == backup))
            .unwrap_or(false);
        if target_matches_backup {
            let _ = fs::remove_file(backup_path);
        }
    }

    Ok(())
}

#[cfg(not(windows))]
fn platform_replace_file(target: &Path, replacement: &Path) -> io::Result<ReplacementReceipt> {
    fs::rename(replacement, target)?;
    Ok(ReplacementReceipt {
        recovery_path: None,
    })
}

fn list_markdown_notes(root: &Path) -> Result<Vec<NoteEntry>, CommandError> {
    let mut notes = Vec::new();
    let mut visited = HashSet::new();
    collect_markdown_notes(root, root, &mut visited, &mut notes)?;
    notes.sort_by_cached_key(|note| note.relative_path.to_lowercase());
    Ok(notes)
}

fn collect_markdown_notes(
    root: &Path,
    directory: &Path,
    visited: &mut HashSet<PathBuf>,
    notes: &mut Vec<NoteEntry>,
) -> Result<(), CommandError> {
    let canonical_directory = fs::canonicalize(directory)
        .map_err(|_| CommandError::new("vault_read_failed", "The vault could not be read."))?;

    if !canonical_directory.starts_with(root) || !visited.insert(canonical_directory.clone()) {
        return Ok(());
    }

    let entries = fs::read_dir(&canonical_directory)
        .map_err(|_| CommandError::new("vault_read_failed", "The vault could not be read."))?;

    for entry in entries {
        let entry = entry
            .map_err(|_| CommandError::new("vault_read_failed", "The vault could not be read."))?;
        let file_type = entry
            .file_type()
            .map_err(|_| CommandError::new("vault_read_failed", "The vault could not be read."))?;

        if file_type.is_symlink() {
            continue;
        }

        if file_type.is_dir() {
            collect_markdown_notes(root, &entry.path(), visited, notes)?;
            continue;
        }

        let path = entry.path();
        let is_markdown = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
        if !file_type.is_file() || !is_markdown {
            continue;
        }

        let canonical_file = fs::canonicalize(&path)
            .map_err(|_| CommandError::new("vault_read_failed", "The vault could not be read."))?;
        if !canonical_file.starts_with(root) {
            continue;
        }

        let relative = canonical_file
            .strip_prefix(root)
            .map_err(|_| CommandError::internal())?;
        let relative_path = relative
            .to_str()
            .ok_or_else(|| {
                CommandError::new(
                    "unsupported_note_path",
                    "A note has a path that cannot be represented safely.",
                )
            })?
            .replace('\\', "/");
        let title = relative
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                CommandError::new(
                    "unsupported_note_path",
                    "A note has a path that cannot be represented safely.",
                )
            })?
            .to_owned();

        notes.push(NoteEntry {
            relative_path,
            title,
        });
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(VaultState::default())
        .invoke_handler(tauri::generate_handler![
            get_runtime_info,
            select_vault,
            reconcile_vault,
            open_note,
            save_note,
            write_recovery_draft,
            list_recovery_drafts,
            read_recovery_draft,
            clear_recovery_draft,
            export_unavailable_recovery_draft,
            delete_unavailable_recovery_draft,
            save_note_as_copy
        ])
        .run(tauri::generate_context!());

    if let Err(error) = result {
        eprintln!("Astian failed to start: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn canonical_temp_root() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("temporary directory should be created");
        let root = fs::canonicalize(temp.path()).expect("root should canonicalize");
        (temp, root)
    }

    fn internal_artifacts(root: &Path) -> Vec<String> {
        fs::read_dir(root)
            .expect("fixture directory should be readable")
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(".astian-"))
            .collect()
    }

    #[test]
    fn runtime_info_uses_package_version_and_host_constants() {
        let info = get_runtime_info().expect("runtime info should be infallible");
        assert_eq!(info.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.platform, std::env::consts::OS);
        assert_eq!(info.architecture, std::env::consts::ARCH);
    }

    #[test]
    fn relative_note_path_rejects_traversal_absolute_and_non_markdown_paths() {
        for path in [
            "",
            "../secret.md",
            "folder/../../secret.md",
            "C:\\secret.md",
            "/secret.md",
            "note.txt",
        ] {
            assert!(
                validate_relative_note_path(path).is_err(),
                "accepted {path}"
            );
        }

        assert!(validate_relative_note_path("projects/Astian.md").is_ok());
    }

    #[test]
    fn scan_and_read_preserve_unicode_crlf_and_bom() {
        let temp = tempfile::tempdir().expect("temporary directory should be created");
        let nested = temp.path().join("Dự án");
        fs::create_dir(&nested).expect("nested directory should be created");
        let original = b"\xef\xbb\xbf# Astian\r\n\r\nGhi chu tieng Viet\r\n";
        fs::write(nested.join("Kế hoạch.MD"), original).expect("fixture should be written");
        fs::write(temp.path().join("ignored.txt"), b"not a note")
            .expect("non-Markdown fixture should be written");

        let root = fs::canonicalize(temp.path()).expect("root should canonicalize");
        let notes = list_markdown_notes(&root).expect("vault should scan");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].relative_path, "Dự án/Kế hoạch.MD");

        let opened = read_markdown_note(&root, &notes[0].relative_path).expect("note should open");
        assert_eq!(opened.content, "# Astian\n\nGhi chu tieng Viet\n");
        assert_eq!(opened.content_hash.len(), 64);
        assert_eq!(opened.line_ending, LineEnding::CrLf);
        assert!(opened.has_utf8_bom);
    }

    #[test]
    fn unchanged_save_does_not_create_or_replace_any_file() {
        let (_temp, root) = canonical_temp_root();
        let original = b"# Astian\r\n";
        fs::write(root.join("note.md"), original).expect("fixture should be written");
        let expected_hash = hash_bytes(original);
        let replace_called = Cell::new(false);

        let result =
            save_markdown_note_with(&root, "note.md", "# Astian\n", &expected_hash, |_, _| {
                replace_called.set(true);
                Err(io::Error::other("replace must not run for a no-op save"))
            })
            .expect("no-op save should succeed");

        assert_eq!(result.status, SaveStatus::Unchanged);
        assert_eq!(
            fs::read(root.join("note.md")).expect("note should read"),
            original
        );
        assert!(!replace_called.get());
        assert!(internal_artifacts(&root).is_empty());
    }

    #[test]
    fn safe_save_preserves_utf8_bom_and_crlf() {
        let (_temp, root) = canonical_temp_root();
        let original = b"\xef\xbb\xbf# Astian\r\n\r\nOld\r\n";
        fs::write(root.join("note.md"), original).expect("fixture should be written");

        let result = save_markdown_note(
            &root,
            "note.md",
            "# Astian\n\nNội dung mới\n",
            &hash_bytes(original),
        )
        .expect("safe save should succeed");
        let expected = b"\xef\xbb\xbf# Astian\r\n\r\nN\xe1\xbb\x99i dung m\xe1\xbb\x9bi\r\n";

        assert_eq!(result.status, SaveStatus::Saved);
        assert_eq!(result.content_hash, hash_bytes(expected));
        assert_eq!(
            fs::read(root.join("note.md")).expect("note should read"),
            expected
        );
        assert!(internal_artifacts(&root).is_empty());
    }

    #[test]
    fn external_change_causes_conflict_without_calling_replace() {
        let (_temp, root) = canonical_temp_root();
        let original = b"original\n";
        let external = b"external\n";
        fs::write(root.join("note.md"), original).expect("fixture should be written");
        let expected_hash = hash_bytes(original);
        fs::write(root.join("note.md"), external).expect("external edit should be written");
        let replace_called = Cell::new(false);

        let error =
            save_markdown_note_with(&root, "note.md", "Astian edit\n", &expected_hash, |_, _| {
                replace_called.set(true);
                Err(io::Error::other("replace must not run during conflict"))
            })
            .expect_err("external edit must conflict");

        assert_eq!(error.code, "external_change_conflict");
        assert!(!replace_called.get());
        assert_eq!(
            fs::read(root.join("note.md")).expect("note should read"),
            external
        );
        assert!(internal_artifacts(&root).is_empty());
    }

    #[test]
    fn mixed_line_endings_are_read_only_when_content_changes() {
        let (_temp, root) = canonical_temp_root();
        let original = b"first\r\nsecond\n";
        fs::write(root.join("note.md"), original).expect("fixture should be written");

        let error = save_markdown_note_with(
            &root,
            "note.md",
            "changed\n",
            &hash_bytes(original),
            |_, _| Err(io::Error::other("replace must not run")),
        )
        .expect_err("mixed line endings should remain read-only");

        assert_eq!(error.code, "mixed_line_endings_read_only");
        assert_eq!(
            fs::read(root.join("note.md")).expect("note should read"),
            original
        );
    }

    #[test]
    fn failed_replace_keeps_original_and_cleans_temporary_file() {
        let (_temp, root) = canonical_temp_root();
        let original = b"original\n";
        fs::write(root.join("note.md"), original).expect("fixture should be written");

        let error = save_markdown_note_with(
            &root,
            "note.md",
            "changed\n",
            &hash_bytes(original),
            |_, _| Err(io::Error::from_raw_os_error(5)),
        )
        .expect_err("replace failure should be reported");

        assert_eq!(error.code, "save_locked");
        assert_eq!(
            fs::read(root.join("note.md")).expect("note should read"),
            original
        );
        assert!(internal_artifacts(&root).is_empty());
    }

    #[test]
    fn recovery_cleanup_failure_does_not_fail_a_verified_save() {
        let (_temp, root) = canonical_temp_root();
        let original = b"original\n";
        fs::write(root.join("note.md"), original).expect("fixture should be written");
        let cleanup_called = Cell::new(false);

        let result = save_markdown_note_and_cleanup(
            &root,
            "note.md",
            "changed\n",
            &hash_bytes(original),
            || {
                cleanup_called.set(true);
                Err(CommandError::new(
                    "recovery_cleanup_failed",
                    "injected cleanup failure",
                ))
            },
        )
        .expect("verified save should remain successful");

        assert_eq!(result.status, SaveStatus::Saved);
        assert!(cleanup_called.get());
        assert_eq!(
            fs::read(root.join("note.md")).expect("saved note should read"),
            b"changed\n"
        );
    }

    #[test]
    fn recovery_copy_is_created_no_clobber_and_normalizes_line_endings() {
        let (_temp, root) = canonical_temp_root();
        let target = root.join("Bản phục hồi.md");

        let copied = create_markdown_copy(&root, &target, "first\r\nsecond\r")
            .expect("recovery copy should be created");

        assert_eq!(copied.relative_path, "Bản phục hồi.md");
        assert_eq!(copied.content, "first\nsecond\n");
        assert_eq!(
            fs::read(&target).expect("copy should read"),
            b"first\nsecond\n"
        );
        assert!(internal_artifacts(&root).is_empty());
        assert_eq!(
            suggested_recovery_copy_name("Dự án/Kế hoạch.md")
                .expect("suggested name should be generated"),
            "Kế hoạch-recovered.md"
        );
    }

    #[test]
    fn recovery_copy_refuses_existing_and_outside_vault_targets() {
        let (temp, root) = canonical_temp_root();
        let existing = root.join("existing.md");
        fs::write(&existing, b"external").expect("existing fixture should write");
        let install_called = Cell::new(false);

        let existing_error = create_markdown_copy_with(&root, &existing, "recovery", |_, _| {
            install_called.set(true);
            Ok(())
        })
        .expect_err("existing target should be refused");
        let outside = temp.path().parent().expect("temp should have a parent");
        let outside_error =
            create_markdown_copy(&root, &outside.join("outside-recovery.md"), "recovery")
                .expect_err("outside target should be refused");

        assert_eq!(existing_error.code, "copy_already_exists");
        assert_eq!(outside_error.code, "copy_outside_vault");
        assert!(!install_called.get());
        assert_eq!(
            fs::read(existing).expect("existing should read"),
            b"external"
        );
        assert!(internal_artifacts(&root).is_empty());
    }

    #[test]
    fn recovery_copy_install_race_preserves_external_file_and_cleans_temp() {
        let (_temp, root) = canonical_temp_root();
        let target = root.join("race.md");

        let error = create_markdown_copy_with(&root, &target, "recovery", |_, target| {
            fs::write(target, b"external")?;
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "injected create race",
            ))
        })
        .expect_err("raced target should not be overwritten");

        assert_eq!(error.code, "copy_already_exists");
        assert_eq!(
            fs::read(&target).expect("external target should read"),
            b"external"
        );
        assert!(internal_artifacts(&root).is_empty());
    }

    #[test]
    fn recovery_copy_cleanup_failure_does_not_fail_a_verified_copy() {
        let (_temp, root) = canonical_temp_root();
        let target = root.join("verified-copy.md");
        let cleanup_called = Cell::new(false);

        let copied = create_markdown_copy_and_cleanup(&root, &target, "recovered", || {
            cleanup_called.set(true);
            Err(CommandError::new(
                "recovery_cleanup_failed",
                "injected cleanup failure",
            ))
        })
        .expect("verified copy should remain successful");

        assert_eq!(copied.content, "recovered");
        assert!(cleanup_called.get());
        assert_eq!(fs::read(target).expect("copy should read"), b"recovered");
    }

    #[test]
    fn recovery_restore_never_overwrites_a_recreated_target() {
        let (_temp, root) = canonical_temp_root();
        let target = root.join("note.md");
        let backup = root.join("note.astian-backup-test.bak");
        fs::write(&backup, b"original").expect("backup should be written");

        recover_backup_after_replace_error(&target, &backup)
            .expect("missing target should recover from backup");
        assert_eq!(fs::read(&target).expect("target should read"), b"original");
        assert!(!backup.exists());

        fs::write(&target, b"external").expect("external target should be written");
        fs::write(&backup, b"original").expect("backup should be written again");
        recover_backup_after_replace_error(&target, &backup)
            .expect("existing target should be preserved");

        assert_eq!(fs::read(&target).expect("target should read"), b"external");
        assert_eq!(
            fs::read(&backup).expect("backup should remain"),
            b"original"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_handle_without_delete_sharing_blocks_replace_without_data_loss() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let (_temp, root) = canonical_temp_root();
        let target = root.join("note.md");
        let original = b"original\n";
        fs::write(&target, original).expect("fixture should be written");
        let _lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&target)
            .expect("fixture lock should open");

        let error = save_markdown_note(&root, "note.md", "changed\n", &hash_bytes(original))
            .expect_err("replace should fail while delete sharing is denied");

        assert_eq!(error.code, "save_locked");
        assert_eq!(fs::read(&target).expect("note should read"), original);
        assert!(internal_artifacts(&root).is_empty());
    }
}
