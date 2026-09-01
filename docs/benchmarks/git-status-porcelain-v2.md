# Git porcelain v2 Windows technical spike

- Date: 2026-09-01
- Conclusion: `go with guardrails`
- Command: `npm run spike:git-status`
- Build: Rust release profile
- Git: `2.55.0.windows.3`
- Target: Windows x86_64
- Reference machine: Intel Core i5-8300H, 16 GB RAM

## Fixture and method

The spike creates and removes a temporary repository. It commits two baseline
files, then creates four status records: a modified Vietnamese filename, a
staged Unicode rename into a directory, a newly staged file modified again in
the worktree, and an untracked Vietnamese filename.

Rust invokes `git.exe` directly using argument arrays. The measured operation
includes canonical repository-root verification followed by
`status --porcelain=v2 -z --branch --untracked-files=all`. Compile time and
fixture creation are outside the timer.

The fixture also captures `git diff --cached --name-status -z` before and after
status inspection, and calls the status API from a nested directory to exercise
the exact root guard.

## Result

| Measurement/check | Result |
| --- | ---: |
| Root verification + status | 153.834 ms |
| Parsed entries | 4 |
| Rename/copy entries | 1 |
| Untracked entries | 1 |
| Pre-staged index preserved | yes |
| Exact repository-root guard | yes |
| Unicode paths observed | yes |

## Guardrails and remaining evidence

- This is a small correctness fixture, not a large-repository latency budget.
- Status buffers complete stdout. Production integration needs an output limit,
  cancellation, background scheduling, and a large repository/10,000-note test.
- No commit, pull, push, credential, remote, divergent-branch, hook-failure, or
  unmerged integration behavior is authorized or proven by this status spike.
- Git executable discovery currently relies on `PATH`; settings/discovery UI and
  version capability policy remain Version 0.5 work.
