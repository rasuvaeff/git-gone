# git-gone

[![CI](https://github.com/rasuvaeff/git-gone/actions/workflows/ci.yml/badge.svg)](https://github.com/rasuvaeff/git-gone/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](./LICENSE-MIT)

**Delete local Git branches whose remote has been deleted — across a single repo
or a whole tree of nested repositories.**

A small, fast, zero-runtime Rust CLI. Finds local branches tracking a remote
ref that no longer exists (`[gone]`) and removes them. The currently
checked-out branch is never deleted.

## Why

`git fetch --prune` cleans up `origin/...` tracking refs but **leaves the local
branches behind**. Over time you accumulate dozens of stale branches. `git-gone`
finishes the job — and, uniquely, can do it for an **entire monorepo** of nested
repositories in one command.

### The killer feature: multi-repo mode

If you work in a workspace where many small git repositories live side by side
(a vendor monorepo, a `~/src` folder of clones, a microservices tree), cleaning
gone branches one repo at a time is tedious:

```bash
$ git gone -r --list
Scanning 58 repositories...

clickhouse-toolkit:
  chore/gitattributes-export-ignore
  chore/package-quality-alignment

domain-monitor:
  chore/package-quality-alignment
  mutation/msi-100

yii3-mcp:
  feat/interceptors
  feature/v1.2-core

29 gone branch(es) across 17 repositories (of 58 scanned).
```

One command, every nested repo. No other `git-gone` tool does this.

### Locale-independent

Many scripts detect gone branches by grepping `[gone]` in `git branch -vv`.
Git **translates** that string: on a Russian locale it prints `[отсутствует]`,
on French `[disparue]`, etc. — the grep silently matches nothing.

`git-gone` resolves every upstream ref through Git's plumbing (one batched
`git cat-file --batch-check` per repository) and treats an unresolvable ref as
gone. Works identically on every locale.

## Install

Requires `git` 2.7 or newer on `PATH` (the tool drives your own `git`).

### One-line (Linux & macOS) — no Rust toolchain required

```bash
curl -fsSL https://raw.githubusercontent.com/rasuvaeff/git-gone/master/scripts/install.sh | sh
```

Override the install location with `INSTALL_DIR=…` (default `/usr/local/bin`;
the default falls back to `~/.local/bin` if not writable and no `sudo`, an
explicitly set `INSTALL_DIR` fails instead of installing elsewhere). To pin both
the installer and its release asset, fetch the installer from the release tag:

```bash
VERSION=v0.2.0
curl -fsSL "https://raw.githubusercontent.com/rasuvaeff/git-gone/${VERSION}/scripts/install.sh" | VERSION="$VERSION" sh
```

The download is verified against the `.sha256` published with the release; a
mismatch aborts the install.

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/rasuvaeff/git-gone/master/scripts/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\git-gone` (override with `$env:INSTALL_DIR`)
and adds it to the user `PATH`. To pin both the installer and its release asset:

```powershell
$env:VERSION = "v0.2.0"
irm "https://raw.githubusercontent.com/rasuvaeff/git-gone/$env:VERSION/scripts/install.ps1" | iex
```

### Cargo

```bash
cargo install git-gone-multi
```

The crate is named `git-gone-multi` (the crate name `git-gone` on crates.io is
taken by an unrelated project), but the **binary it installs is `git-gone`** —
the `git gone` alias works as usual. If you already have the other `git-gone`
installed via cargo, the binaries collide; add `--force` to replace it.

Or straight from the repository:

```bash
cargo install --git https://github.com/rasuvaeff/git-gone
```

### Manual download

Prebuilt binaries for `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`,
`x86_64-apple-darwin`, and `x86_64-pc-windows-msvc` are on the
[Releases](https://github.com/rasuvaeff/git-gone/releases) page. The Linux
build links against the system glibc; on other Linux targets (aarch64, musl)
install from source.

### Use it as `git gone`

Git discovers `git-gone` on your `PATH` automatically — after install you can
type either `git gone` or `git-gone`.

## Usage

### Single repository (default)

```
$ git gone
Fetching and pruning remote-tracking refs...
Gone branches (2):
  feature-x (2 unmerged commits)
  refactor/auth
Delete these 2 branches? (y/N) y
Deleted branch: feature-x (was 4f2a1c9)
Deleted branch: refactor/auth (was 91b0e77)
Recover a deleted branch with: git branch <name> <sha>
Done.
```

`(N unmerged commits)` marks commits no other ref can reach — what the forced
delete would actually lose (reflog aside). A merged branch shows no suffix.

### Multi-repo (`--recursive`)

```
$ git gone -r --yes
Deleted clickhouse-toolkit/chore/gitattributes-export-ignore (was 4f2a1c9)
Deleted clickhouse-toolkit/chore/package-quality-alignment (was 0d19ab3)
Deleted domain-monitor/mutation/msi-100 (was 77c5e21)
Done.
```

Scan a different tree without changing directory:

```bash
git gone -r --root ~/src --dry-run
```

### Flags

| Flag | Short | Effect |
|---|---|---|
| `--recursive` | `-r` | Scan subdirectories and operate on **all** nested git repositories |
| `--root PATH` | | Scan this directory tree with `-r` (default: current directory) |
| `--depth N` | | Max scan depth with `-r` (`0` = the scan root only, `1` = direct subdirs; default unlimited). The scan root itself is always inspected |
| `--exclude a,b,c` | | Comma-separated dir names to skip during scan (replaces `git config gone.exclude` and the default `target,vendor,node_modules,.cache,build,dist`) |
| `--jobs N` | `-j` | Repositories fetched in parallel with `-r` (default: available CPUs, capped at 8) |
| `--safe` | | Pre-filter branches not merged into `HEAD`, then use `git branch -d` (default is forced `-D`) |
| `--squash-safe` | | Pre-filter branches whose final tree does not exactly match a commit reachable from `HEAD`, then delete matches with `git branch -D`; conflicts with `--safe` |
| `--protect a,b` | | Branch name patterns (`*` wildcard) that are never deletion candidates; adds to `git config gone.protect` |
| `--json` | | Machine-readable JSON report (implies report mode: no deletion; conflicts with `--yes`) |
| `--include-reasons` | | Add upstream and unmerged-commit details to `--json` output |
| `--allow-external-git-dir` | | With `-r`, allow linked-worktree metadata outside `--root` (use only for trusted trees) |
| `--list` | `-l` | Print gone branches, delete nothing |
| `--dry-run` | `-n` | Print what would be deleted, delete nothing |
| `--yes` | `-y` | Skip the confirmation prompt (for scripts/CI) |
| `--no-fetch` | | Skip the initial `git fetch --prune` |

`--depth`, `--exclude`, `--jobs`, `--root`, and `--allow-external-git-dir` are
only meaningful with `-r` and are rejected without it. `--include-reasons`
requires `--json`.

### Configuration (`git config`)

| Key | Meaning |
|---|---|
| `gone.protect` | Branch name patterns (`*` wildcard, multi-valued or comma-separated) that are never deletion candidates. Repo-local, global and system levels all apply; `--protect` adds to them. Skipped branches are announced: `Protected branches skipped: develop` |
| `gone.exclude` | Directory names skipped by the `-r` scan. Precedence: `--exclude` flag > `gone.exclude` > built-in default — each level replaces the one below |

```bash
git config --global gone.protect 'develop,release/*'
```

When run **without** `--yes` and stdin is not a TTY (e.g. piped), `git-gone`
refuses to delete and exits `1`. Pass `--yes` to force.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success — deleted, or nothing to delete, or a report mode (`--list`/`--dry-run`/`--json`) |
| `1` | Aborted at the prompt, refused without a TTY, a branch could not be deleted, `-r` could not refresh a repository before deletion, a repository could not be inspected or a directory could not be read, or a fatal error |

Under `-r --json` a repository that could not be inspected is reported in the
`error` field **and** exits `1` — the document is still printed in full, so
`git gone -r --json > report.json` keeps its payload while `&&` chains and CI
steps stop on an incomplete scan. A directory that could not be read is a
warning on stderr and exits `1` for the same reason: it may hide repositories.
A fatal error before any report is produced (not a repository, `git fetch`
failed in single-repo mode) exits `1` with no JSON on stdout.

### JSON output (`--json`)

For CI / scripting. A document is **always** printed, including when nothing is
gone — safe to pipe into `jq`.

**The JSON shape is a public contract under SemVer:** fields are only added in
minor versions, never renamed or removed outside a major. Safe to build CI on.
`skipped` carries branch names that could not be parsed (non-UTF-8/TAB) — they
reach the machine consumer, not only stderr. Protected branches are excluded
from `gone` entirely.

Pass `--include-reasons` to add a `reasons` array without changing the existing
fields. Each entry carries the branch, the upstream ref that did not resolve, and
the number of commits unique to that branch (`null` if the best-effort count
fails):

```json
{"gone": ["feature-x"], "skipped": [], "reasons": [{"branch": "feature-x", "upstream": "refs/remotes/origin/feature-x", "unmerged_commits": 2}]}
```

Single repository:

```json
{"gone": ["feature-x", "refactor/auth"], "skipped": []}
```

Multi-repo (`-r --json`) — `error` is `null` for a healthy repository and carries
the failure message for one that could not be inspected (never silently reported
as clean). `fetch_error` is `null` when `git fetch --prune` succeeded (or was
skipped via `--no-fetch`); when it carries a message, detection still ran but on
possibly stale tracking refs — `"gone": []` next to a `fetch_error` is *stale*,
not verified-clean. A `fetch_error` alone does not change the exit code in a
report mode, but the repository is excluded from a destructive `-r` run:

```json
{
  "repositories": [
    {"path": "clickhouse-toolkit", "gone": ["chore/x", "feat/y"], "skipped": [], "error": null, "fetch_error": null},
    {"path": "specification", "gone": [], "skipped": [], "error": null, "fetch_error": "`git fetch --prune` exited with status 128 in …"}
  ],
  "total_gone": 2
}
```

## How it works

- **Fetch first:** unless `--no-fetch` is passed, `git fetch --prune` runs before
  detection — without it a branch deleted on the remote elsewhere still has a
  local tracking ref and would not be seen as gone. Under `-r` a failing fetch is
  still recorded in the JSON `fetch_error` field and reports can show its stale
  candidates, but deletion skips that repository and exits `1`.
- **Safe mode:** `--safe` pre-filters branches not merged into `HEAD`, so its report
  and confirmation only contain branches it can delete with `git branch -d`. The default
  remains `git branch -D`, because a gone remote branch cannot be merged there later.
- **Squash-safe mode:** `--squash-safe` accepts a branch only when its final Git tree
  exactly matches a commit reachable from `HEAD`. This covers squash merges without
  guessing from patch similarity; it deletes the verified ref with `git branch -D` because
  Git's ancestry-only `-d` check cannot recognize a squash merge.
- **Fetches run in parallel under `-r`** (`-j`, default: available CPUs capped
  at 8) — the fetch phase is network-bound and dominates the wall clock. A
  progress line (`[12/58] pkg-a`) is shown when stderr is a terminal. Detection
  is local and sequential, so the report order is always deterministic.
- **Single-repo:** `git for-each-ref` lists local branches with their upstream;
  all upstreams are then resolved in one `git cat-file --batch-check` call — an
  upstream that does not resolve to a commit is gone. No `[gone]` text matching,
  no locale dependency, and two `git` processes per repository regardless of
  branch count.
- **Multi-repo:** recursively walks the directory tree, treating any directory
  containing `.git` as a repository. A meta-repo that itself wraps per-package
  repos (a monorepo with independent package git repos) is descended into, not
  treated as a leaf. Dot-directories and the default exclude list are skipped.
  Git metadata (including `commondir`) must resolve inside the scan root; an
  escaped gitfile or symlink is reported as a failed scan instead of being used.
- All git invocations go through `std::process::Command` — no `git2`/`gix`
  crate, no libgit2 linkage. A single self-contained binary (~800 KB) with two
  dependencies (`clap`, `anyhow`) that uses your own `git`, config and credentials.

### Notes

- A branch checked out in **any** worktree of the repository is never a deletion
  candidate — `git worktree list --porcelain` is consulted, not just the current
  `HEAD`, so a branch checked out elsewhere is neither reported nor attempted.
- Deletions print the short SHA they dropped (`Deleted branch: feature-x (was
  4f2a1c9)`): `git branch -D` is a forced delete, and the SHA is what makes it
  recoverable via `git reflog` / `git branch <name> <sha>` — a restore hint is
  printed after every run that deleted something.
- The listing marks branches whose commits no other ref can reach —
  `feature-x (2 unmerged commits)` — so the decision to delete is informed, not
  blind. The count is best-effort: if it cannot be computed, no suffix is shown.
- Branches matching `gone.protect` / `--protect` are removed from the candidates
  and announced (`Protected branches skipped: develop`) — never silently.
- Branch names with a TAB or with non-UTF-8 bytes cannot be round-tripped back to
  Git, so they are **skipped with a warning** — never reported as gone and never
  deleted. Silently reporting them would be worse: a lossy name resolves to no
  ref, which looks exactly like "the upstream is gone".
- A ref whose name starts with `-` (creatable via `git update-ref`, not via
  `git branch`) is handled: the name is passed after `--`.

## Security

- **`-r` runs `git` inside every directory it finds, so it runs *their* config.**
  Git executes repository-local settings such as `core.sshCommand` during
  `git fetch`; a repository you did not write is therefore executable content.
  Do not run `git gone -r` over a tree containing untrusted clones — or pass
  `--no-fetch`, which limits the run to local ref inspection.
  (`safe.directory` only protects you from repositories owned by *another* user;
  it does not help with an untrusted repository you cloned yourself.)
- **Recursive repository metadata is contained by the scan root.** A nested
  `.git` gitfile, symlink, or `commondir` that resolves outside the requested
  tree is rejected and makes the scan fail; it cannot redirect deletion to an
  unrelated repository. Linked worktrees whose Git metadata lives outside the
  scan root must be handled from their owning tree instead. For a trusted layout
  that requires this, `--allow-external-git-dir` is an explicit opt-in.
- **A failed fetch blocks deletion in that repository.** `--json`, `--list`, and
  `--dry-run` can report stale tracking refs with `fetch_error`; normal and
  `--yes` recursive runs never delete from such a repository and exit `1`.
- **Credential prompts are disabled under `-r`** (`GIT_TERMINAL_PROMPT=0`): one
  repository with a private remote would otherwise block the whole batch on a
  password prompt. Single-repo mode keeps prompts enabled. Note the limit: this
  covers Git's own prompts (HTTP credentials); `ssh` itself may still ask for a
  key passphrase or host-key confirmation. For a fully unattended batch use an
  `ssh-agent`, or `--no-fetch` to skip fetching entirely.
- **Deletion is forced** (`git branch -D`) by design — the remote is already
  gone, and `-d` would refuse unmerged branches that can never be merged now.
  The printed `(was <sha>)` is the recovery handle; branch tips also stay in the
  reflog until it expires.
- **Nothing is passed through a shell.** Every git call is `std::process::Command`
  with an argument vector, and branch names are passed after `--`.
- **Installer downloads are checksum-verified.** Both `install.sh` and
  `install.ps1` fetch the published `<asset>.sha256` and refuse to install on a
  mismatch, so a `curl … | sh` pipeline fails closed.
- **No `unsafe`**: the crate is `#![forbid(unsafe_code)]`.

## Comparison

| Tool | Lang | **Multi-repo** | Locale-safe | Confirmation | `git gone` alias |
|---|---|:---:|:---:|:---:|:---:|
| **this `git-gone`** | Rust | **yes (`-r`)** | yes | yes | yes |
| [swsnr/git-gone][swsnr] | Rust | no | yes | explicit `prune` subcommand | yes |
| [git-delete-merged-branches][gdm] | Python | no | yes | yes | yes |

[swsnr]: https://codeberg.org/swsnr/git-gone
[gdm]: https://github.com/hartwork/git-delete-merged-branches

> Note: the crate name `git-gone` on crates.io is taken by `swsnr/git-gone`
> (since 2018). This project is published as **`git-gone-multi`** (the binary is
> still `git-gone`) and differentiates by the multi-repo mode.

## Development

```bash
cargo fmt
cargo clippy --all-targets --locked -- -D warnings
cargo test --all --locked
cargo run -- --list
cargo run -- -r --dry-run
```

Rust 1.85+ (edition 2024; see `rust-version` in `Cargo.toml`).

Layout: `cli.rs` (flags), `git.rs` (every `git` invocation and the detection
parser), `scan.rs` (directory walk), `report.rs` (per-repo outcome and totals),
`json.rs` (document builders), `run.rs` (the two modes), `main.rs` (dispatch).
Unit tests live next to the code they cover; `tests/integration.rs` drives the
built binary against real repositories.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](./LICENSE-APACHE))
- MIT License ([LICENSE-MIT](./LICENSE-MIT))

at your option.
