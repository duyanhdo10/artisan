use super::{hash_bytes, CommandError, LineEnding, OpenedNote};
use std::{
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
};
use tempfile::Builder as TempFileBuilder;

#[cfg(windows)]
use std::{os::windows::ffi::OsStrExt, ptr};
#[cfg(windows)]
use windows_sys::Win32::Globalization::{
    CompareStringOrdinal, NormalizationC, NormalizeString, CSTR_EQUAL,
};

const MAX_COMPONENT_UTF16: usize = 255;
const MAX_LEGACY_PATH_UTF16: usize = 259;
const INTERNAL_SIBLING_NAME_BUDGET: usize = 64;

pub(super) fn create_note(
    root: &Path,
    parent_relative_path: &str,
    requested_name: &str,
) -> Result<OpenedNote, CommandError> {
    create_note_with(
        root,
        parent_relative_path,
        requested_name,
        |source, target| fs::hard_link(source, target),
    )
}

fn create_note_with<F>(
    root: &Path,
    parent_relative_path: &str,
    requested_name: &str,
    install: F,
) -> Result<OpenedNote, CommandError>
where
    F: Fn(&Path, &Path) -> io::Result<()>,
{
    let stored_name = validate_note_name(requested_name)?;
    let parent = canonical_note_parent(root, parent_relative_path)?;
    validate_path_lengths(&parent, &stored_name)?;

    let mut temporary = TempFileBuilder::new()
        .prefix(".astian-create-")
        .suffix(".tmp")
        .tempfile_in(&parent)
        .map_err(|_| {
            CommandError::new(
                "note_create_prepare_failed",
                "The note could not be prepared safely.",
            )
        })?;
    temporary
        .write_all(&[])
        .and_then(|_| temporary.flush())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| {
            CommandError::new(
                "note_create_write_failed",
                "The note could not be written safely.",
            )
        })?;

    // This latest enumeration happens under the vault write coordinator, after
    // the payload is durable and immediately before the no-clobber install.
    ensure_name_available(&parent, &stored_name)?;
    let (temporary_file, temporary_path) = temporary.keep().map_err(|_| {
        CommandError::new(
            "note_create_prepare_failed",
            "The note could not be prepared safely.",
        )
    })?;
    drop(temporary_file);
    let _temporary_cleanup = CleanupPath(temporary_path.clone());
    let target = parent.join(&stored_name);

    install(&temporary_path, &target).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            name_collision_error()
        } else {
            CommandError::new(
                "note_create_install_failed",
                "The note could not be installed safely.",
            )
        }
    })?;

    let bytes = fs::read(&target).map_err(|_| {
        CommandError::new(
            "note_create_verification_failed",
            "Astian created the note but could not verify it.",
        )
    })?;
    if !bytes.is_empty() {
        return Err(CommandError::new(
            "note_create_verification_failed",
            "Astian created the note but could not verify it.",
        ));
    }

    let relative = target.strip_prefix(root).map_err(|_| {
        CommandError::new("note_parent_unavailable", "The note folder is unavailable.")
    })?;
    let relative_path = relative
        .to_str()
        .ok_or_else(|| {
            CommandError::new(
                "invalid_note_name",
                "The note path cannot be represented safely.",
            )
        })?
        .replace('\\', "/");

    Ok(OpenedNote {
        relative_path,
        content: String::new(),
        content_hash: hash_bytes(&bytes),
        line_ending: LineEnding::None,
        has_utf8_bom: false,
    })
}

fn validate_note_name(requested_name: &str) -> Result<String, CommandError> {
    if requested_name.is_empty()
        || requested_name == "."
        || requested_name == ".."
        || requested_name
            .chars()
            .any(|character| character <= '\u{1f}' || "<>:\"/\\|?*".contains(character))
        || requested_name
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
        || requested_name
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
        || requested_name.ends_with('.')
    {
        return Err(invalid_name_error());
    }

    let normalized = normalize_nfc(requested_name)?;
    let lower = normalized.to_lowercase();
    if lower.starts_with(".astian-") {
        return Err(CommandError::new(
            "reserved_note_name",
            "That note name is reserved by Astian.",
        ));
    }

    let suffix_start = normalized.len().saturating_sub(3);
    let stored_name = if normalized
        .get(suffix_start..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".md"))
    {
        format!("{}.md", &normalized[..suffix_start])
    } else {
        format!("{normalized}.md")
    };
    let stem = &stored_name[..stored_name.len() - 3];
    if stem.is_empty() {
        return Err(invalid_name_error());
    }
    if is_reserved_device_basename(stem) {
        return Err(CommandError::new(
            "reserved_note_name",
            "That filename is reserved by Windows.",
        ));
    }
    if stored_name.encode_utf16().count() > MAX_COMPONENT_UTF16 {
        return Err(CommandError::new(
            "note_name_too_long",
            "The note name is too long for a Windows vault.",
        ));
    }
    Ok(stored_name)
}

fn is_reserved_device_basename(stem: &str) -> bool {
    let basename = stem.split('.').next().unwrap_or(stem).to_uppercase();
    matches!(basename.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || is_numbered_device(&basename, "COM")
        || is_numbered_device(&basename, "LPT")
}

fn is_numbered_device(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        matches!(
            suffix,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
    })
}

fn canonical_note_parent(root: &Path, relative: &str) -> Result<PathBuf, CommandError> {
    let relative_path = Path::new(relative);
    if !relative.is_empty()
        && !relative_path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(parent_error());
    }
    let candidate = if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative_path)
    };
    let parent = fs::canonicalize(candidate).map_err(|_| parent_error())?;
    if !parent.starts_with(root) || !parent.is_dir() {
        return Err(parent_error());
    }
    Ok(parent)
}

fn ensure_name_available(parent: &Path, requested_name: &str) -> Result<(), CommandError> {
    let entries = fs::read_dir(parent).map_err(|_| parent_error())?;
    for entry in entries {
        let entry = entry.map_err(|_| parent_error())?;
        let Some(existing_name) = entry.file_name().to_str().map(str::to_owned) else {
            // Fail closed because Astian cannot prove ordinal non-collision for
            // an unrepresentable entry in this namespace.
            return Err(name_collision_error());
        };
        let normalized_existing = normalize_nfc(&existing_name)?;
        if ordinal_eq_ignore_case(&normalized_existing, requested_name)? {
            return Err(name_collision_error());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn normalize_nfc(value: &str) -> Result<String, CommandError> {
    let source: Vec<u16> = value.encode_utf16().collect();
    let source_len = i32::try_from(source.len()).map_err(|_| invalid_name_error())?;
    // SAFETY: source points to a live UTF-16 buffer. A null output with length
    // zero is the documented sizing call.
    let required = unsafe {
        NormalizeString(
            NormalizationC,
            source.as_ptr(),
            source_len,
            ptr::null_mut(),
            0,
        )
    };
    if required <= 0 {
        return Err(invalid_name_error());
    }
    let mut output = vec![0_u16; required as usize];
    // SAFETY: output has the capacity reported by the sizing call and both
    // buffers remain live for the duration of NormalizeString.
    let written = unsafe {
        NormalizeString(
            NormalizationC,
            source.as_ptr(),
            source_len,
            output.as_mut_ptr(),
            required,
        )
    };
    if written <= 0 {
        return Err(invalid_name_error());
    }
    output.truncate(written as usize);
    String::from_utf16(&output).map_err(|_| invalid_name_error())
}

#[cfg(not(windows))]
fn normalize_nfc(value: &str) -> Result<String, CommandError> {
    Ok(value.to_owned())
}

#[cfg(windows)]
fn ordinal_eq_ignore_case(left: &str, right: &str) -> Result<bool, CommandError> {
    let left: Vec<u16> = left.encode_utf16().collect();
    let right: Vec<u16> = right.encode_utf16().collect();
    let left_len = i32::try_from(left.len()).map_err(|_| invalid_name_error())?;
    let right_len = i32::try_from(right.len()).map_err(|_| invalid_name_error())?;
    // SAFETY: both pointers reference live UTF-16 buffers with explicit lengths.
    let comparison =
        unsafe { CompareStringOrdinal(left.as_ptr(), left_len, right.as_ptr(), right_len, 1) };
    if comparison == 0 {
        return Err(CommandError::internal());
    }
    Ok(comparison == CSTR_EQUAL)
}

#[cfg(not(windows))]
fn ordinal_eq_ignore_case(left: &str, right: &str) -> Result<bool, CommandError> {
    Ok(left.eq_ignore_ascii_case(right))
}

fn validate_path_lengths(parent: &Path, stored_name: &str) -> Result<(), CommandError> {
    let parent_units = path_utf16_len(parent);
    let target_units = parent_units + 1 + stored_name.encode_utf16().count();
    let temporary_units = parent_units + 1 + INTERNAL_SIBLING_NAME_BUDGET;
    if target_units > MAX_LEGACY_PATH_UTF16 || temporary_units > MAX_LEGACY_PATH_UTF16 {
        return Err(CommandError::new(
            "note_name_too_long",
            "The note path is too long for a Windows vault.",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn path_utf16_len(path: &Path) -> usize {
    path.as_os_str().encode_wide().count()
}

#[cfg(not(windows))]
fn path_utf16_len(path: &Path) -> usize {
    path.to_string_lossy().encode_utf16().count()
}

fn invalid_name_error() -> CommandError {
    CommandError::new(
        "invalid_note_name",
        "Choose a valid Windows filename for the note.",
    )
}

fn parent_error() -> CommandError {
    CommandError::new("note_parent_unavailable", "The note folder is unavailable.")
}

fn name_collision_error() -> CommandError {
    CommandError::new(
        "name_collision",
        "A file or folder already uses that name. Choose another name.",
    )
}

struct CleanupPath(PathBuf);

impl Drop for CleanupPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn note_name_validation_rejects_unsafe_and_reserved_names() {
        for name in [
            "",
            ".",
            "..",
            " leading",
            "\u{00a0}leading",
            "trailing ",
            "trailing.",
            "a/b",
            "a\\b",
            "a:b",
            ".astian-write-x",
            "CON",
            "nul.txt",
            "COM9.log",
            "COM¹.log",
            "LPT².data",
            ".md",
        ] {
            assert!(validate_note_name(name).is_err(), "accepted {name:?}");
        }

        let too_long = "a".repeat(253);
        let error = validate_note_name(&too_long).expect_err("256 UTF-16 units should fail");
        assert_eq!(error.code, "note_name_too_long");
    }

    #[test]
    fn note_name_normalizes_nfc_and_canonicalizes_extension() {
        assert_eq!(
            validate_note_name("Ke\u{301} hoa\u{323}ch.MD").expect("name should normalize"),
            "Ké hoạch.md"
        );
        assert_eq!(
            validate_note_name("meeting.v1").expect("extension should append"),
            "meeting.v1.md"
        );
    }

    #[test]
    fn create_note_succeeds_with_unicode_and_leaves_no_internal_artifact() {
        let (_temp, root) = canonical_temp_root();
        let created = create_note(&root, "", "Kế hoạch").expect("note should be created");

        assert_eq!(created.relative_path, "Kế hoạch.md");
        assert_eq!(created.content, "");
        assert_eq!(created.content_hash, hash_bytes(&[]));
        assert_eq!(created.line_ending, LineEnding::None);
        assert_eq!(
            fs::read(root.join("Kế hoạch.md")).expect("note should read"),
            b""
        );
        assert!(internal_artifacts(&root).is_empty());
    }

    #[test]
    fn create_note_refuses_case_and_normalization_collisions() {
        let (_temp, root) = canonical_temp_root();
        fs::write(root.join("Plan.MD"), b"existing").expect("fixture should write");
        let case_error = create_note(&root, "", "plan").expect_err("case collision expected");
        assert_eq!(case_error.code, "name_collision");

        fs::write(root.join("Ke\u{301} hoạch.md"), b"existing")
            .expect("decomposed fixture should write");
        let nfc_error =
            create_note(&root, "", "Ké hoạch").expect_err("normalization collision expected");
        assert_eq!(nfc_error.code, "name_collision");
    }

    #[test]
    fn create_note_rejects_traversal_and_missing_parent() {
        let (_temp, root) = canonical_temp_root();
        for parent in ["..", "missing", "folder/../../outside"] {
            let error =
                create_note(&root, parent, "safe").expect_err("unsafe parent should be rejected");
            assert_eq!(error.code, "note_parent_unavailable");
        }
    }

    #[test]
    fn install_race_preserves_external_target_and_cleans_temporary_file() {
        let (_temp, root) = canonical_temp_root();
        let error = create_note_with(&root, "", "race", |_, target| {
            fs::write(target, b"external")?;
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "injected create race",
            ))
        })
        .expect_err("race should fail closed");

        assert_eq!(error.code, "name_collision");
        assert_eq!(
            fs::read(root.join("race.md")).expect("target should read"),
            b"external"
        );
        assert!(internal_artifacts(&root).is_empty());
    }

    #[test]
    fn verification_failure_is_typed_and_does_not_claim_target_is_missing() {
        let (_temp, root) = canonical_temp_root();
        let error = create_note_with(&root, "", "verify", |_, target| {
            fs::write(target, b"unexpected")
        })
        .expect_err("verification should fail");

        assert_eq!(error.code, "note_create_verification_failed");
        assert_eq!(
            fs::read(root.join("verify.md")).expect("target should remain"),
            b"unexpected"
        );
        assert!(internal_artifacts(&root).is_empty());
    }
}
