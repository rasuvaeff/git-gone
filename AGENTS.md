# AGENTS.md — git-gone

Guidance for AI agents working on this package. Read before changing code.

## What this is

`git-gone` is a small Rust CLI that deletes local Git branches whose remote has
been deleted (`[gone]`). Its differentiator vs. other `git-gone` tools is the
**multi-repo mode** (`-r`/`--recursive`): scan a directory tree and clean gone
branches across every nested Git repository in one command — built for the
`rasuvaeff/*` monorepo layout where the root is itself a git repo and each
package is an independent git repo. It is the first non-PHP package in the
monorepo: it follows the monorepo conventions where they apply (documentation,
CHANGELOG, SHA-pinned CI, golden rules, `AGENTS.md`/`CLAUDE.md`) and ignores the
PHP/Composer-specific rules (templates, Testo, Psalm, Docker `composer:2`,
`bin/dev`, BSD-3-Clause). The public surface is the CLI flags and the binary
name (`git-gone`, also invocable as `git gone` because Git discovers `git-*` on
`PATH`).

## Golden rules

1. **Verification is mandatory.** Never claim "done" without a fresh green
   `cargo fmt --all -- --check` + `cargo clippy --all-targets -- -D warnings`
   + `cargo test --all`. "Should work" does not count.
2. **No suppressions.** No `#[allow(...)]` / `#![allow(...)]` to silence clippy,
   no `#[allow(dead_code)]` to hide unused code. Fix the root cause.
3. **Detection must stay locale-independent.** Never grep the `[gone]` string
   from `git branch -vv` — Git translates it (`[отсутствует]`, `[disparue]`,
   …) and the match silently fails on every non-English locale. Always resolve
   the upstream ref through plumbing — one batched `git cat-file --batch-check`
   per repository with `<upstream>^{commit}` queries (` missing` is untranslated
   plumbing output) — and treat an unresolvable ref as gone. The currently
   checked-out branch must never be a deletion candidate.
4. **Preserve the public contract.** Update `README.md` **and `README.ru.md`**
   (both languages, same commit), `llms.txt`, `CHANGELOG.md`, and tests with any
   change to flags or behavior.

## Commands

Rust is available on the host — no Docker needed.

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run -- --list
cargo run -- --dry-run
cargo build --release
```

Full release gate:

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --all --locked
```

CI passes `--locked` too: the release build already did, and the mismatch let
`Cargo.lock` drift pass CI and fail the release.

`Cargo.lock` is committed (this is a binary crate, not a library).

## Invariants & gotchas

- **Locale-independent detection** — see Golden rule 3. This is the whole reason
  the tool exists; shell one-liners (`git branch -vv | grep '\[gone\]'`) break on
  non-English systems.
- **No branch checked out in any worktree is deleted.** Resolved via
  `git worktree list --porcelain` (untranslated) plus `git symbolic-ref --short
  HEAD` as the detached-HEAD-safe fallback, and filtered out before prompting.
  Filtering only the current `HEAD` used to make a branch checked out in another
  worktree a candidate whose deletion could not succeed.
- **Non-TTY without `--yes` exits 1.** Piping into `git-gone`
  (e.g. `echo y | git-gone`) refuses to delete, to avoid accidental mass
  deletion in scripts. Pass `--yes` explicitly for CI.
- **`git branch -D` (forced delete).** Gone branches have already lost their
  remote; `-D` matches user intent. `--safe` pre-filters branches not merged into
  `HEAD`, then switches to `-d` (merged-only) — do not change the default. The deletion message is printed by
  this tool with the short SHA read *before* the delete (`(was 4f2a1c9)`), and
  Git's own stdout is silenced: Git translates its message, and the SHA is the
  recovery handle for a forced delete.
- **`GIT_TERMINAL_PROMPT=0` under `-r`.** A batch run must not block on a
  credential prompt from one repository. Single-repo mode keeps prompts enabled —
  there the user is present. Known limit (documented, not "fixed"): this covers
  Git's own prompts (HTTP credentials); `ssh` can still prompt for a key
  passphrase or host-key confirmation. Forcing `-oBatchMode=yes` via
  `GIT_SSH_COMMAND` would override the user's own `core.sshCommand` — the
  documented answer is `ssh-agent` or `--no-fetch`, not an env override.
- **A failed recursive fetch is visible in the report but never eligible for deletion.**
  `RepoReport.fetch_error` records a failed `git fetch --prune` under `-r`, and
  `--json` exposes it as the `fetch_error` field: detection then ran on possibly
  stale tracking refs, and `"gone":[]` next to a `fetch_error` is stale, not
  verified-clean. `--json`, `--list`, and `--dry-run` keep that diagnostic report;
  normal and `--yes` runs exclude the repository from deletion and exit `1`.
  Covered by `recursive_json_reports_a_failed_fetch` and
  `recursive_does_not_delete_after_a_failed_fetch`.
- **Bare repositories are rejected by the printed answer, not the exit status.**
  `git rev-parse --is-inside-work-tree` prints `false` with exit 0 in a bare
  repo, so `ensure_in_repo` compares stdout against `true`. Covered by
  `a_bare_repository_is_rejected`.
- **`--json` conflicts with `--yes`** (clap `conflicts_with`): `--json` implies
  report mode, so `--yes` would be silently ignored — rejecting the combination
  beats surprising a script author.
- **JSON reasons are additive.** `--include-reasons` requires `--json` and adds
  `reasons` entries with branch, unresolved upstream, and best-effort unique
  commit count. Never change the existing `gone` array shape; it is a SemVer
  contract. Keep `Detection` upstream metadata in sync when protection or a
  checked-out worktree removes a candidate.
- **Upstream resolution is batched.** One `git cat-file --batch-check` process
  per repository resolves every upstream (stdin written from a thread — writing
  all queries first and only then reading deadlocks once a pipe buffer fills).
  Any answer other than ` missing` counts as alive: the conservative direction —
  an uncertain branch is kept, never deleted. Do not go back to one
  `git rev-parse` per branch.
- **Threat model: `-r` executes repo-local git config.** `git fetch` honours
  settings like `core.sshCommand` from the repository it runs in, so scanning a
  tree of untrusted clones executes their code. This is documented in both
  READMEs; `--no-fetch` is the mitigation. Do not "fix" it in code — it is how
  git works, and using the user's own git is the point.
- **`git fetch --prune` runs before detection** unless `--no-fetch` — in **both**
  modes. Without it a branch deleted on the remote elsewhere still has a local
  tracking ref and is never reported. This was regressed once by the multi-repo
  refactor (fetch survived only in `run_multi`) and is now covered by
  `default_fetches_and_prunes_before_detecting`. Any new test that passes
  `--no-fetch` must not be the *only* coverage of a code path.
- **A repository that cannot be inspected is never reported as clean.**
  `RepoReport.result` is a `Result<Detection, String>` — the type, not a comment,
  is what prevents an empty `gone` from passing as clean. `--json` exposes the
  failure as the `error` field, the human output prints `Warning: could not
  inspect …`, and the exit code becomes `1` **in every mode, `--json` included**
  (the document is still printed in full). Never go back to `unwrap_or_default()`,
  and never let the JSON path return `0` on a failure: that contradiction is what
  a CI consumer of the report silently trips over.
- **The same rule covers the directory scan.** `scan::Scan::errors` records
  unreadable directories and entries; they are warned about and set the exit code.
  An unreadable directory can hide every repository underneath it, so it must
  never look like "nothing here".
- **And branch names that cannot be parsed.** A name with non-UTF-8 bytes or an
  embedded TAB goes to `Detection::skipped` and is warned about — never into
  `gone`. Lossy decoding produces a name that resolves to no ref, which is
  indistinguishable from "the upstream is gone": a live branch would be reported
  as deletable. Covered by `parse_ref_line` unit tests and
  `a_non_utf8_branch_name_is_skipped_not_reported_as_gone`.
- **Branch names go to `git` after `--`.** `git branch -D -- <name>`: a ref named
  `-x` cannot be created by `git branch`, but `git update-ref` and `git fetch`
  can produce one, and without the separator Git parses it as an option.
- **`--json` always prints a document**, including the empty case
  (`{"gone":[]}` / `"repositories":[]`) — consumers pipe stdout into `jq`.
- **Binary name `git-gone` ⇒ `git gone`.** Git discovers `git-*` on `PATH`, so
  the installed binary is invocable as a subcommand. The crate is named
  `git-gone-multi` (the crates.io name `git-gone` is taken by an unrelated
  project) and the binary name is pinned by the `[[bin]]` section in
  `Cargo.toml` — never remove that section: without it the binary would take
  the crate name and break the `git gone` alias. The integration tests' 
  `CARGO_BIN_EXE_git-gone` env var is keyed by the *bin* name, not the crate name.
- **The fetch phase is parallel; everything else is not.** `fetch_all` runs
  `git fetch --prune` `--jobs` at a time (default: CPUs capped at 8) because it
  is network-bound; results are stored by index so the report order stays
  deterministic regardless of completion order. Detection stays sequential.
  Under `Prompt::Deny` fetch stderr is **captured**, not inherited — parallel
  git processes would otherwise interleave on the terminal — and becomes part
  of the error message. The progress line is printed only when stderr is a
  terminal, one `eprint!` call per update (stderr is locked per call, so
  workers cannot garble each other mid-line).
- **Protected branches are filtered, and the filter is announced.**
  `gone.protect` (multi-valued git config, all levels) plus `--protect` are
  additive; patterns use a local anchored `*`-wildcard matcher
  (`wildcard_match` in `run.rs` — no glob crate). Removed candidates are
  printed (`Protected branches skipped: …`) — never silent — and are excluded
  from JSON `gone` entirely. `gone.exclude` is different: it sits between the
  built-in default and `--exclude`, each level *replacing* the one below.
- **The unmerged-commit suffix is best-effort.** `git rev-list --count
  <ref> --not --exclude=<ref> --all` counts commits no other ref reaches;
  `None` (failure) renders as no suffix, never as an error. Branch names cannot
  contain `*`, `?` or `[`, so the refname is always a literal for `--exclude`.
- **The JSON shape is a SemVer contract** (stated in both READMEs and
  `llms.txt`): fields are only added in minors. `skipped` is part of the
  document — unparsable names must reach machine consumers, not only stderr.
- **Root and deletion modes.** `--root PATH` selects the recursive scan tree
  and requires `-r`. `--safe` removes branches not merged into `HEAD` from the
  report before switching deletion from `git branch -D` to `git branch -d`. The
  `--squash-safe` instead accepts only an exact final-tree match in `HEAD` history,
  then uses `git branch -D`; never weaken this to patch-similarity heuristics. The
  default stays forced to preserve the original public contract.
- **Minimum git is 2.7** (`git worktree list --porcelain`) — documented in the
  install sections; do not adopt newer git features without bumping that line.
- **Multi-repo scan (`-r`) descends past a found repo.** Unlike a submodule
  leaf scan, finding `.git` does NOT stop descent: a meta-repo that wraps
  per-package repos (the `rasuvaeff/*` layout, where the root is itself a git
  repo and each package is an independent git repo) must be entered so all
  nested repositories are discovered. Dot-directories are always skipped; the
  default exclude list (`target,vendor,node_modules,.cache,build,dist`) is
  **replaced wholesale** by `--exclude` (not appended to). `--json` forces
  report mode (no deletion) so CI can consume the report safely.
- **Recursive Git metadata stays inside the scan root.** A discovered `.git`
  directory, gitfile, symlink, and (when present) `commondir` are canonicalized
  before Git is invoked. Any path outside the requested root is a scan error,
  never a repository candidate: otherwise a crafted nested gitfile can make
  `git branch -D` mutate an unrelated repository. Internal gitfiles remain
  supported for nested submodules; a linked worktree whose metadata is outside
  the root must be scanned from its owning tree unless the user explicitly passes
  `--allow-external-git-dir` for a trusted layout. Covered by
  `recursive_rejects_a_gitfile_that_points_outside_the_scan_root`.
- **Branch names containing TAB (`\t`) are not parsed.** The
  `for-each-ref --format=…%09…` output splits on TAB, so such a line yields three
  fields instead of two. Deletion of these names stays unsupported, but they are
  skipped with a warning rather than mis-split — see the `Detection::skipped`
  invariant above.
- **All git invocations go through `std::process::Command`.** No `git2` crate,
  no libgit2 linkage: the tool uses the user's own `git` binary, config, and
  credentials, and stays a single static binary with two deps (`clap`, `anyhow`).
- **Module layout.** `cli.rs` (flags + `exclude_list`), `git.rs` (every `git`
  invocation, `Detection`, `parse_ref_line`), `scan.rs` (`Scan`, walk, `rel_path`),
  `report.rs` (`RepoReport` + aggregates), `json.rs` (document builders, unit
  tested against exact strings), `run.rs` (single/multi modes), `main.rs`
  (dispatch only). Keep the printing out of `git.rs` and the git calls out of
  `run.rs`.
- **`main` returns `ExitCode`**, it does not call `std::process::exit` — the
  latter skips destructors and buffer flushes. Modes return `u8`.
- Code: `#![forbid(unsafe_code)]`, `cargo fmt` formatting, explicit types, `?` for
  error propagation. Unit tests live in a `#[cfg(test)] mod tests` next to the code
  they cover; `tests/integration.rs` drives the built binary.
- `examples/` is part of the public contract when added: keep scripts runnable.
- **CI workflows are SHA-pinned.** Every `uses:` in `.github/workflows/*.yml`
  references a 40-char commit SHA with a `# vN` trailing comment. Only
  `actions/checkout` is used as an action: GitHub-hosted runners already ship
  Rust stable with `rustfmt` and `clippy`, and the MSRV job installs `1.85.0`
  via `rustup toolchain install` + `cargo +1.85.0` directly — so no
  `dtolnay/rust-toolchain` action is needed (keeps the CI free of moving-branch
  SHA dependencies). Never add floating tags/branches. Updates go through
  Dependabot (`.github/dependabot.yml`, ecosystems `github-actions` + `cargo`,
  with a 7-day cooldown), which bumps the SHA and preserves the comment.
  Workflows carry `permissions: { contents: read }` at workflow level,
  `persist-credentials: false` on every `actions/checkout`, and a
  `concurrency:` group that cancels stale runs on the same ref. The **release
  workflow** (`release.yml`) keeps `contents: read` at workflow level and raises
  it to `contents: write` **on the job only**, with the justification as a
  trailing comment on that same line (zizmor's `undocumented-permissions` only
  accepts it there): there is no more granular permission for creating a
  Release and uploading assets. Verify with `zizmor --persona=auditor .github/`
  before release — must report no `unpinned-uses`, `excessive-permissions`, or
  `artipacked`. The remaining `superfluous-actions` note on
  `softprops/action-gh-release` is accepted: `gh release` in a script step would
  trade a SHA-pinned action for hand-rolled upload logic.
- **`shell: bash` on cross-platform `run:` steps that use `$VAR`.** The Windows
  runner defaults to pwsh, where `"$TARGET"` expands to nothing — the release
  build step silently loses its `--target`. Either set `shell: bash` or use
  `$env:VAR` with `shell: pwsh` (as the zip-packaging step does).
- **Release assets carry a `.sha256`.** Both installers fetch it and refuse to
  install on a mismatch, so a `curl … | sh` pipeline fails closed. If the release
  workflow stops publishing those files, the installers break — that is deliberate.
  `shasum -a 256` (not `sha256sum`) is used: macOS runners have no `sha256sum`.
- **Installer contract.** An explicitly set `INSTALL_DIR` is honored or the
  install fails — only the default `/usr/local/bin` may fall back to
  `~/.local/bin`. `VERSION=vX.Y.Z` (`$env:VERSION` on Windows) selects a release
  asset instead of `releases/latest`; it does NOT pin installer code fetched from
  `master`. A reproducible install fetches `scripts/install.*` from the same
  version tag and passes `VERSION` to that script, as shown in both READMEs.
- **`cargo package` ignores `.gitattributes` `export-ignore`.** The dev-only file
  list for the crate archive lives in `Cargo.toml` `exclude`; keep the two in sync
  and verify with `cargo package --list`.

## When you finish

- Update `README.md` **and `README.ru.md`** (both languages, same commit) and
  `llms.txt`; update `CHANGELOG.md` when flags or behavior change.
- Re-run the full release gate
  (`cargo fmt --check && cargo clippy -- -D warnings && cargo test --all`)
  and paste the output.
- If `.github/workflows/*.yml` changed, run `zizmor --persona=auditor .github/`
  and confirm it is clean.
