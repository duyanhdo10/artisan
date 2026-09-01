# SQLite FTS5 benchmark — 10,000 Vietnamese notes

- Date: 2026-09-01
- Conclusion: `go with guardrails`
- Command: `npm run benchmark:search`
- Build: Rust release profile, bundled SQLite 3.53.2
- Target: Windows x86_64
- CPU: Intel64 Family 6 Model 158 Stepping 10, 8 logical processors
- Reference machine: Intel Core i5-8300H, 16 GB RAM

## Fixture and method

The deterministic fixture contains 10,000 in-memory Markdown documents totaling
13,841,682 UTF-8 bytes. Documents have Vietnamese titles and paragraphs,
frontmatter, tags, wiki links, and a varied 3–20 paragraph distribution. The
benchmark database is created in a temporary directory and removed afterward;
no generated note or database is written into the project or a user vault.

Initial rebuild deletes and inserts all note/FTS rows in one transaction and
runs FTS5 optimize. Warm search measures 1,600 queries across eight accented and
unaccented Vietnamese query forms, returning up to 20 ranked results. Incremental
measurement updates one existing note 200 times, one transaction per update.

## Results

| Measurement | Result | Technical-spike budget |
| --- | ---: | ---: |
| Fixture generation | 38.27 ms | informational |
| Initial rebuild | 3,261.49 ms | background work; no fixed spike budget |
| Warm search p50 | 18.317 ms | 200 ms p95 |
| Warm search p95 | 33.301 ms | 200 ms p95 |
| Warm search p99 | 50.770 ms | informational |
| Incremental update p50 | 40.578 ms | 500 ms |
| Incremental update p95 | 51.489 ms | 500 ms |
| Incremental update p99 | 64.646 ms | informational |
| Database size after checkpoint | 46,665,728 bytes | informational |

All correctness probes returned results for both accented and unaccented forms.
The measured search p95 is about 6× below the 200 ms budget, and incremental
update p95 is about 9× below the 500 ms budget.

## Guardrails and remaining evidence

- This isolates SQLite/FTS behavior. It does not include filesystem scan,
  Markdown parsing, watcher scheduling, IPC, React render, or snippet paint.
- The fixture content distribution is deterministic and representative enough
  for architecture selection, but it is not a corpus sampled from user data.
- Initial rebuild must run in the background with progress/cancellation before
  Version 0.4; 3.26 seconds must not block the editor.
- Production integration still needs app-local database lifecycle, per-vault
  identity, watcher-driven incremental scheduling, corruption replacement, and
  a 10,000-file end-to-end benchmark from disk.
- Benchmark numbers are local reference evidence, not a cross-machine guarantee.

A post-installer-fix verification run from Cargo `examples` also passed, with
warm search p95 `38.235 ms` and incremental update p95 `82.745 ms`. Initial
rebuild varied to `6,010.68 ms`, reinforcing that rebuild belongs in a
background pipeline; both interactive budgets remained comfortably satisfied.
