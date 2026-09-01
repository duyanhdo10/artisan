use std::{
    ffi::{OsStr, OsString},
    fmt, fs, io,
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GitBranchStatus {
    pub oid: Option<String>,
    pub head: Option<String>,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitEntryKind {
    Ordinary,
    RenamedOrCopied,
    Unmerged,
    Untracked,
    Ignored,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitStatusEntry {
    pub kind: GitEntryKind,
    pub index_status: char,
    pub worktree_status: char,
    pub submodule_status: Option<String>,
    pub path: String,
    pub original_path: Option<String>,
    pub rename_or_copy_score: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GitRepositoryStatus {
    pub branch: GitBranchStatus,
    pub entries: Vec<GitStatusEntry>,
}

#[derive(Debug)]
pub enum GitStatusError {
    GitUnavailable(io::Error),
    Filesystem(io::Error),
    NotRepository,
    RepositoryRootMismatch,
    ProcessFailed,
    MalformedOutput,
    UnsupportedPathEncoding,
}

impl GitStatusError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::GitUnavailable(_) => "git_unavailable",
            Self::Filesystem(_) => "git_filesystem_failed",
            Self::NotRepository => "git_not_repository",
            Self::RepositoryRootMismatch => "git_repository_root_mismatch",
            Self::ProcessFailed => "git_process_failed",
            Self::MalformedOutput => "git_status_malformed",
            Self::UnsupportedPathEncoding => "git_path_encoding_unsupported",
        }
    }
}

impl fmt::Display for GitStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::GitUnavailable(_) => "Git for Windows is unavailable.",
            Self::Filesystem(_) => "The vault path could not be verified for Git.",
            Self::NotRepository => "The vault is not a Git working tree.",
            Self::RepositoryRootMismatch => {
                "Git integration requires the repository root to exactly match the vault root."
            }
            Self::ProcessFailed => "Git status could not be read.",
            Self::MalformedOutput => "Git returned an unsupported status response.",
            Self::UnsupportedPathEncoding => {
                "Git returned a path that cannot be represented safely."
            }
        })
    }
}

impl std::error::Error for GitStatusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::GitUnavailable(error) | Self::Filesystem(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitClient {
    executable: OsString,
}

impl Default for GitClient {
    fn default() -> Self {
        Self::new("git")
    }
}

impl GitClient {
    pub fn new(executable: impl AsRef<OsStr>) -> Self {
        Self {
            executable: executable.as_ref().to_owned(),
        }
    }

    pub fn version(&self) -> Result<String, GitStatusError> {
        let output = self.run(None, ["--version"])?;
        if !output.status.success() {
            return Err(GitStatusError::ProcessFailed);
        }
        let version = std::str::from_utf8(&output.stdout)
            .map_err(|_| GitStatusError::MalformedOutput)?
            .trim();
        if version.is_empty() {
            return Err(GitStatusError::MalformedOutput);
        }
        Ok(version.to_owned())
    }

    pub fn status(&self, vault_root: &Path) -> Result<GitRepositoryStatus, GitStatusError> {
        let canonical_vault = fs::canonicalize(vault_root).map_err(GitStatusError::Filesystem)?;
        let repository_root = self.repository_root(&canonical_vault)?;
        if repository_root != canonical_vault {
            return Err(GitStatusError::RepositoryRootMismatch);
        }

        let output = self.run(
            Some(&canonical_vault),
            [
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--untracked-files=all",
            ],
        )?;
        if !output.status.success() {
            return Err(GitStatusError::ProcessFailed);
        }
        parse_porcelain_v2_z(&output.stdout)
    }

    fn repository_root(&self, vault_root: &Path) -> Result<PathBuf, GitStatusError> {
        let output = self.run(
            Some(vault_root),
            ["rev-parse", "--path-format=absolute", "--show-toplevel"],
        )?;
        if !output.status.success() {
            return Err(GitStatusError::NotRepository);
        }
        let root = std::str::from_utf8(&output.stdout)
            .map_err(|_| GitStatusError::UnsupportedPathEncoding)?
            .trim_end_matches(['\r', '\n']);
        if root.is_empty() || root.contains(['\r', '\n', '\0']) {
            return Err(GitStatusError::MalformedOutput);
        }
        fs::canonicalize(root).map_err(GitStatusError::Filesystem)
    }

    fn run<I, S>(&self, working_tree: Option<&Path>, arguments: I) -> Result<Output, GitStatusError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(&self.executable);
        if let Some(root) = working_tree {
            command.arg("-C").arg(root);
        }
        command
            .args(arguments)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);

        command.output().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                GitStatusError::GitUnavailable(error)
            } else {
                GitStatusError::Filesystem(error)
            }
        })
    }
}

pub fn parse_porcelain_v2_z(output: &[u8]) -> Result<GitRepositoryStatus, GitStatusError> {
    let mut status = GitRepositoryStatus::default();
    let mut records = output.split(|byte| *byte == 0).peekable();

    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        match record.first().copied() {
            Some(b'#') => parse_header(record, &mut status.branch)?,
            Some(b'1') => status.entries.push(parse_ordinary(record)?),
            Some(b'2') => {
                let original_path = records.next().ok_or(GitStatusError::MalformedOutput)?;
                status.entries.push(parse_renamed(record, original_path)?);
            }
            Some(b'u') => status.entries.push(parse_unmerged(record)?),
            Some(b'?') => status
                .entries
                .push(parse_simple(record, GitEntryKind::Untracked, '?')?),
            Some(b'!') => status
                .entries
                .push(parse_simple(record, GitEntryKind::Ignored, '!')?),
            _ => return Err(GitStatusError::MalformedOutput),
        }
    }

    Ok(status)
}

fn parse_header(record: &[u8], branch: &mut GitBranchStatus) -> Result<(), GitStatusError> {
    let header = std::str::from_utf8(record).map_err(|_| GitStatusError::MalformedOutput)?;
    if let Some(value) = header.strip_prefix("# branch.oid ") {
        branch.oid = (value != "(initial)").then(|| value.to_owned());
    } else if let Some(value) = header.strip_prefix("# branch.head ") {
        branch.detached = value == "(detached)";
        branch.head = (!branch.detached).then(|| value.to_owned());
    } else if let Some(value) = header.strip_prefix("# branch.upstream ") {
        branch.upstream = Some(value.to_owned());
    } else if let Some(value) = header.strip_prefix("# branch.ab ") {
        let mut counts = value.split(' ');
        branch.ahead = Some(parse_count(counts.next(), '+')?);
        branch.behind = Some(parse_count(counts.next(), '-')?);
        if counts.next().is_some() {
            return Err(GitStatusError::MalformedOutput);
        }
    }
    Ok(())
}

fn parse_count(value: Option<&str>, prefix: char) -> Result<u64, GitStatusError> {
    value
        .and_then(|value| value.strip_prefix(prefix))
        .and_then(|value| value.parse().ok())
        .ok_or(GitStatusError::MalformedOutput)
}

fn parse_ordinary(record: &[u8]) -> Result<GitStatusEntry, GitStatusError> {
    let fields = split_fields(record, 9)?;
    let (index_status, worktree_status) = parse_xy(fields[1])?;
    Ok(GitStatusEntry {
        kind: GitEntryKind::Ordinary,
        index_status,
        worktree_status,
        submodule_status: Some(parse_ascii(fields[2])?.to_owned()),
        path: parse_path(fields[8])?,
        original_path: None,
        rename_or_copy_score: None,
    })
}

fn parse_renamed(record: &[u8], original_path: &[u8]) -> Result<GitStatusEntry, GitStatusError> {
    let fields = split_fields(record, 10)?;
    let (index_status, worktree_status) = parse_xy(fields[1])?;
    let score = parse_ascii(fields[8])?;
    if !matches!(score.as_bytes().first(), Some(b'R' | b'C'))
        || !score.as_bytes()[1..].iter().all(u8::is_ascii_digit)
    {
        return Err(GitStatusError::MalformedOutput);
    }
    Ok(GitStatusEntry {
        kind: GitEntryKind::RenamedOrCopied,
        index_status,
        worktree_status,
        submodule_status: Some(parse_ascii(fields[2])?.to_owned()),
        path: parse_path(fields[9])?,
        original_path: Some(parse_path(original_path)?),
        rename_or_copy_score: Some(score.to_owned()),
    })
}

fn parse_unmerged(record: &[u8]) -> Result<GitStatusEntry, GitStatusError> {
    let fields = split_fields(record, 11)?;
    let (index_status, worktree_status) = parse_xy(fields[1])?;
    Ok(GitStatusEntry {
        kind: GitEntryKind::Unmerged,
        index_status,
        worktree_status,
        submodule_status: Some(parse_ascii(fields[2])?.to_owned()),
        path: parse_path(fields[10])?,
        original_path: None,
        rename_or_copy_score: None,
    })
}

fn parse_simple(
    record: &[u8],
    kind: GitEntryKind,
    marker: char,
) -> Result<GitStatusEntry, GitStatusError> {
    if record.get(1) != Some(&b' ') {
        return Err(GitStatusError::MalformedOutput);
    }
    Ok(GitStatusEntry {
        kind,
        index_status: marker,
        worktree_status: marker,
        submodule_status: None,
        path: parse_path(&record[2..])?,
        original_path: None,
        rename_or_copy_score: None,
    })
}

fn split_fields(record: &[u8], count: usize) -> Result<Vec<&[u8]>, GitStatusError> {
    let fields: Vec<_> = record.splitn(count, |byte| *byte == b' ').collect();
    if fields.len() != count || fields.iter().any(|field| field.is_empty()) {
        return Err(GitStatusError::MalformedOutput);
    }
    Ok(fields)
}

fn parse_xy(value: &[u8]) -> Result<(char, char), GitStatusError> {
    if value.len() != 2 || !value.is_ascii() {
        return Err(GitStatusError::MalformedOutput);
    }
    Ok((char::from(value[0]), char::from(value[1])))
}

fn parse_ascii(value: &[u8]) -> Result<&str, GitStatusError> {
    if !value.is_ascii() {
        return Err(GitStatusError::MalformedOutput);
    }
    std::str::from_utf8(value).map_err(|_| GitStatusError::MalformedOutput)
}

fn parse_path(value: &[u8]) -> Result<String, GitStatusError> {
    let path = std::str::from_utf8(value)
        .map_err(|_| GitStatusError::UnsupportedPathEncoding)?
        .to_owned();
    let bytes = path.as_bytes();
    let has_windows_drive_prefix =
        bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if path.is_empty()
        || path.starts_with(['/', '\\'])
        || has_windows_drive_prefix
        || Path::new(&path)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(GitStatusError::MalformedOutput);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZERO_HASH: &str = "0000000000000000000000000000000000000000";

    #[test]
    fn parser_handles_headers_all_entry_kinds_unicode_spaces_and_rename_order() {
        let ordinary =
            format!("1 .M N... 100644 100644 100644 {ZERO_HASH} {ZERO_HASH} Dự án/Ghi chú.md\0");
        let renamed = format!(
            "2 R. N... 100644 100644 100644 {ZERO_HASH} {ZERO_HASH} R100 Tên mới.md\0Tên cũ.md\0"
        );
        let unmerged = format!("u UU N... 100644 100644 100644 100644 {ZERO_HASH} {ZERO_HASH} {ZERO_HASH} conflict.md\0");
        let input = format!(
            "# branch.oid {ZERO_HASH}\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -3\0{ordinary}{renamed}{unmerged}? chưa theo dõi.md\0"
        );

        let status = parse_porcelain_v2_z(input.as_bytes()).expect("status should parse");

        assert_eq!(status.branch.head.as_deref(), Some("main"));
        assert_eq!(status.branch.ahead, Some(2));
        assert_eq!(status.branch.behind, Some(3));
        assert_eq!(status.entries.len(), 4);
        assert_eq!(status.entries[0].path, "Dự án/Ghi chú.md");
        assert_eq!(status.entries[1].path, "Tên mới.md");
        assert_eq!(
            status.entries[1].original_path.as_deref(),
            Some("Tên cũ.md")
        );
        assert_eq!(status.entries[2].kind, GitEntryKind::Unmerged);
        assert_eq!(status.entries[3].kind, GitEntryKind::Untracked);
    }

    #[test]
    fn parser_ignores_future_headers_and_rejects_malformed_records_and_paths() {
        assert!(parse_porcelain_v2_z(b"# future.header value\0? valid.md\0").is_ok());
        for input in [
            b"1 too-short\0".as_slice(),
            b"2 R. N... 1 1 1 a b R100 new.md\0".as_slice(),
            b"? ../outside.md\0".as_slice(),
            b"? C:/absolute.md\0".as_slice(),
            b"x unknown\0".as_slice(),
        ] {
            assert!(matches!(
                parse_porcelain_v2_z(input),
                Err(GitStatusError::MalformedOutput)
            ));
        }
    }

    #[test]
    fn parser_rejects_non_utf8_paths() {
        assert!(matches!(
            parse_porcelain_v2_z(b"? \xff.md\0"),
            Err(GitStatusError::UnsupportedPathEncoding)
        ));
    }

    #[test]
    fn missing_git_has_a_stable_error_code() {
        let client = GitClient::new("astian-definitely-missing-git-executable.exe");
        let error = client.version().expect_err("missing Git should fail");
        assert_eq!(error.code(), "git_unavailable");
    }

    #[cfg(windows)]
    #[test]
    fn windows_git_status_preserves_pre_staged_state_and_unicode_paths() {
        let fixture = GitFixture::new();
        let cached_before = fixture.git_output(["diff", "--cached", "--name-status", "-z"]);
        let status = GitClient::default()
            .status(&fixture.root)
            .expect("Git status should parse");
        let cached_after = fixture.git_output(["diff", "--cached", "--name-status", "-z"]);

        assert_eq!(cached_after, cached_before);
        assert_eq!(status.branch.head.as_deref(), Some("main"));
        assert!(status.entries.iter().any(|entry| {
            entry.path == "Ghi chú.md" && entry.index_status == '.' && entry.worktree_status == 'M'
        }));
        assert!(status.entries.iter().any(|entry| {
            entry.path == "Dự án/Tên mới.md"
                && entry.original_path.as_deref() == Some("old name.md")
                && entry.kind == GitEntryKind::RenamedOrCopied
        }));
        assert!(status.entries.iter().any(|entry| {
            entry.path == "staged file.md"
                && entry.index_status == 'A'
                && entry.worktree_status == 'M'
        }));
        assert!(status.entries.iter().any(|entry| {
            entry.path == "chưa theo dõi.md" && entry.kind == GitEntryKind::Untracked
        }));
    }

    #[cfg(windows)]
    #[test]
    fn nested_vault_is_rejected_when_repository_root_does_not_match() {
        let fixture = GitFixture::new();
        let nested = fixture.root.join("Dự án");
        let error = GitClient::default()
            .status(&nested)
            .expect_err("nested vault should not inherit parent repository");
        assert!(matches!(error, GitStatusError::RepositoryRootMismatch));
    }

    #[cfg(windows)]
    #[test]
    fn directory_without_git_metadata_is_reported_as_not_a_repository() {
        let temp = tempfile::tempdir().expect("non-repository fixture should open");
        let error = GitClient::default()
            .status(temp.path())
            .expect_err("plain directory should not have Git status");

        assert!(matches!(error, GitStatusError::NotRepository));
        assert_eq!(error.code(), "git_not_repository");
    }

    #[cfg(windows)]
    struct GitFixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
    }

    #[cfg(windows)]
    impl GitFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("Git fixture should open");
            let root = fs::canonicalize(temp.path()).expect("Git fixture should canonicalize");
            run_fixture_git(&root, ["init", "--initial-branch=main", "--template="]);
            run_fixture_git(&root, ["config", "core.autocrlf", "false"]);
            fs::write(root.join("Ghi chú.md"), "ban đầu\n").expect("Unicode note should write");
            fs::write(root.join("old name.md"), "rename me\n").expect("rename note should write");
            run_fixture_git(&root, ["add", "--", "Ghi chú.md", "old name.md"]);
            run_fixture_git(
                &root,
                [
                    "-c",
                    "user.name=Astian Test",
                    "-c",
                    "user.email=astian@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-m",
                    "fixture",
                    "--no-verify",
                ],
            );

            fs::write(root.join("Ghi chú.md"), "đã sửa\n").expect("tracked note should modify");
            fs::create_dir(root.join("Dự án")).expect("Unicode directory should write");
            run_fixture_git(&root, ["mv", "--", "old name.md", "Dự án/Tên mới.md"]);
            fs::write(root.join("staged file.md"), "staged\n").expect("staged note should write");
            run_fixture_git(&root, ["add", "--", "staged file.md"]);
            fs::write(root.join("staged file.md"), "staged then edited\n")
                .expect("staged note should modify");
            fs::write(root.join("chưa theo dõi.md"), "untracked\n")
                .expect("untracked note should write");

            Self { _temp: temp, root }
        }

        fn git_output<const N: usize>(&self, arguments: [&str; N]) -> Vec<u8> {
            fixture_git_output(&self.root, arguments)
        }
    }

    #[cfg(windows)]
    fn run_fixture_git<const N: usize>(root: &Path, arguments: [&str; N]) {
        let output = fixture_git_output(root, arguments);
        std::hint::black_box(output);
    }

    #[cfg(windows)]
    fn fixture_git_output<const N: usize>(root: &Path, arguments: [&str; N]) -> Vec<u8> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .output()
            .expect("Git for Windows should run");
        assert!(
            output.status.success(),
            "Git fixture command failed with status {:?}",
            output.status.code()
        );
        output.stdout
    }
}
