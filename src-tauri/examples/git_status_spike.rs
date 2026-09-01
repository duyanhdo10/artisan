//! Windows correctness fixture for direct Git porcelain v2 status inspection.

use astian_lib::git_status::{GitClient, GitEntryKind, GitStatusError};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let root = fs::canonicalize(temp.path())?;
    run_git(&root, ["init", "--initial-branch=main", "--template="])?;
    run_git(&root, ["config", "core.autocrlf", "false"])?;
    fs::write(root.join("Ghi chú.md"), "ban đầu\n")?;
    fs::write(root.join("old name.md"), "rename me\n")?;
    run_git(&root, ["add", "--", "Ghi chú.md", "old name.md"])?;
    run_git(
        &root,
        [
            "-c",
            "user.name=Astian Spike",
            "-c",
            "user.email=astian@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "fixture",
            "--no-verify",
        ],
    )?;

    fs::write(root.join("Ghi chú.md"), "đã sửa\n")?;
    fs::create_dir(root.join("Dự án"))?;
    run_git(&root, ["mv", "--", "old name.md", "Dự án/Tên mới.md"])?;
    fs::write(root.join("staged file.md"), "staged\n")?;
    run_git(&root, ["add", "--", "staged file.md"])?;
    fs::write(root.join("staged file.md"), "staged then edited\n")?;
    fs::write(root.join("chưa theo dõi.md"), "untracked\n")?;

    let cached_before = git_output(&root, ["diff", "--cached", "--name-status", "-z"])?;
    let client = GitClient::default();
    let started = Instant::now();
    let status = client.status(&root)?;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let cached_after = git_output(&root, ["diff", "--cached", "--name-status", "-z"])?;
    if cached_after != cached_before {
        return Err("status inspection changed the pre-staged index".into());
    }

    let nested_error = client
        .status(&root.join("Dự án"))
        .expect_err("nested root must be rejected");
    if !matches!(nested_error, GitStatusError::RepositoryRootMismatch) {
        return Err("nested root returned the wrong error".into());
    }
    let renamed = status
        .entries
        .iter()
        .filter(|entry| entry.kind == GitEntryKind::RenamedOrCopied)
        .count();
    let untracked = status
        .entries
        .iter()
        .filter(|entry| entry.kind == GitEntryKind::Untracked)
        .count();

    println!("Astian Git porcelain v2 technical spike");
    println!("git_version={}", client.version()?);
    println!("status_elapsed_ms={elapsed_ms:.3}");
    println!(
        "branch={}",
        status.branch.head.as_deref().unwrap_or("detached")
    );
    println!("entries={}", status.entries.len());
    println!("renamed_entries={renamed}");
    println!("untracked_entries={untracked}");
    println!("pre_staged_state_preserved=true");
    println!("exact_repository_root_guard=true");
    println!("unicode_paths_observed=true");

    Ok(())
}

fn run_git<const N: usize>(
    root: &Path,
    arguments: [&str; N],
) -> Result<(), Box<dyn std::error::Error>> {
    let output = git_output(root, arguments)?;
    std::hint::black_box(output);
    Ok(())
}

fn git_output<const N: usize>(
    root: &Path,
    arguments: [&str; N],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err("Git fixture command failed".into());
    }
    Ok(output.stdout)
}
