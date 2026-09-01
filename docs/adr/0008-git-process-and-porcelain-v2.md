# ADR 0008: Invoke Git directly and parse porcelain v2 with NUL delimiters

- Status: Accepted for the Windows technical spike
- Date: 2026-09-01

## Context

Astian needs conservative Git status without taking ownership of credentials,
changing a user's pre-staged index, interpreting localized human output, or
allowing a vault nested inside a larger repository to affect files outside the
vault.

## Options considered

1. Invoke Git through a shell and parse short/human status output.
2. Link a Git implementation such as libgit2.
3. Invoke the user-installed Git executable with an argument array and parse
   porcelain version 2 with NUL-delimited paths.

## Decision

Astian uses option 3 for the spike.

- Rust launches `git.exe` directly with `std::process::Command`; no command is
  assembled as a shell string. Standard input is closed, terminal prompts are
  disabled, optional Git locks are disabled for read-only inspection, and the
  Windows process is created without a console window.
- Before status, `rev-parse --path-format=absolute --show-toplevel` is
  canonicalized and must equal the canonical vault root. A nested vault cannot
  inherit a parent repository in Astian 1.0.
- Status uses `status --porcelain=v2 -z --branch --untracked-files=all`.
  The parser handles ordinary, rename/copy, unmerged, untracked, and ignored
  records; unknown optional headers are ignored as required by the format.
- Rename/copy records treat the first path as the destination and the following
  NUL field as the original path. Paths are never split on whitespace or parsed
  from localized text.
- Missing Git, non-repository, root mismatch, process failure, malformed output,
  and unsupported path encoding remain distinct stable error categories. Git
  stderr is not forwarded to UI/logs because it may contain paths or remote data.
- Status inspection is read-only and tests compare the staged index before and
  after inspection. Future commit/pull/push operations require separate
  preflight and policy; this decision does not authorize them.

## Consequences

- Astian follows the user's Git for Windows installation and credential tooling
  without storing tokens or reimplementing Git behavior.
- Git must be installed and discoverable, unless a later setting adds an explicit
  executable path. Version/capability checks must produce actionable UI state.
- Windows paths are required to decode as UTF-8 in porcelain output for this
  spike. Unsupported output fails closed instead of guessing an encoding.
- `Command::output` buffers the complete status response. Large-repository
  cancellation, output limits, background scheduling, and typed IPC/UI remain
  Version 0.5 integration work.
- The release fixture measured canonical root verification plus status at
  153.834 ms on Git `2.55.0.windows.3`. The approach is `go with guardrails`;
  methodology and limits are recorded in
  `docs/benchmarks/git-status-porcelain-v2.md`.
