use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_git-gone");

fn commit_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("GIT_AUTHOR_NAME", "test"),
        ("GIT_AUTHOR_EMAIL", "test@example.com"),
        ("GIT_COMMITTER_NAME", "test"),
        ("GIT_COMMITTER_EMAIL", "test@example.com"),
    ]
}

fn git(work: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(work)
        .envs(commit_env())
        .output()
        .unwrap_or_else(|e| panic!("running `git {}`: {e}", args.join(" ")));
    assert!(
        out.status.success(),
        "`git {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn gone_bin(work: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(BIN)
        .args(args)
        .current_dir(work)
        .output()
        .expect("running git-gone binary");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Builds a repo with a `feature-x` branch that has been pushed, then deleted on the remote
/// and pruned locally — so `feature-x` is `[gone]`.
fn repo_with_gone_branch() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let bare = root.join("remote.git");
    let work = root.join("repo");

    git(root, &["init", "--bare", "-b", "main", &path(&bare)]);
    git(root, &["clone", &path(&bare), &path(&work)]);

    fs::write(work.join("a.txt"), "a").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "init"]);
    git(&work, &["push", "-u", "origin", "main"]);

    git(&work, &["checkout", "-b", "feature-x"]);
    fs::write(work.join("x.txt"), "x").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "x"]);
    git(&work, &["push", "-u", "origin", "feature-x"]);

    git(&work, &["checkout", "main"]);
    git(&work, &["push", "origin", "--delete", "feature-x"]);
    git(&work, &["fetch", "--prune"]);

    (tmp, work)
}

/// Adds a gone branch whose final file tree was reproduced on `main` by a different commit,
/// modeling a squash merge. Its changes are present, but its commit is not an ancestor of HEAD.
fn add_squash_merged_gone_branch(work: &Path) {
    git(work, &["checkout", "-b", "squash-x"]);
    fs::write(work.join("squash.txt"), "same contents\n").unwrap();
    git(work, &["add", "."]);
    git(work, &["commit", "-m", "feature implementation"]);
    git(work, &["push", "-u", "origin", "squash-x"]);

    git(work, &["checkout", "main"]);
    fs::write(work.join("squash.txt"), "same contents\n").unwrap();
    git(work, &["add", "."]);
    git(work, &["commit", "-m", "squash feature implementation"]);
    git(work, &["push"]);
    git(work, &["push", "origin", "--delete", "squash-x"]);
    git(work, &["fetch", "--prune"]);
}

fn path(p: &Path) -> String {
    p.to_str().unwrap().to_string()
}

fn branches(work: &Path) -> Vec<String> {
    git(
        work,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
    )
    .lines()
    .map(String::from)
    .collect()
}

#[test]
fn deletes_gone_branch_with_yes() {
    let (_tmp, work) = repo_with_gone_branch();

    let (ok, out, err) = gone_bin(&work, &["--yes", "--no-fetch"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains("Deleted branch: feature-x"), "stdout: {out}");
    // The short SHA keeps a forced delete recoverable from the reflog.
    assert!(
        out.contains("Deleted branch: feature-x (was "),
        "deletion must report the commit it dropped; stdout: {out}"
    );
    assert!(
        err.contains("Recover a deleted branch with: git branch"),
        "the restore hint must follow a deletion; stderr: {err}"
    );
    assert!(!branches(&work).iter().any(|b| b == "feature-x"));
}

/// `feature-x` carries a commit no other ref reaches — the listing must say what the
/// forced delete would lose. A gone branch that is fully merged shows no suffix.
#[test]
fn listing_shows_unmerged_commit_count() {
    let (_tmp, work) = repo_with_gone_branch();
    // `merged-x` points at main's tip with a gone upstream: deletable, nothing lost.
    git(&work, &["branch", "merged-x", "main"]);
    git(&work, &["config", "branch.merged-x.remote", "origin"]);
    git(
        &work,
        &["config", "branch.merged-x.merge", "refs/heads/merged-x"],
    );

    let (ok, _out, err) = gone_bin(&work, &["--list", "--no-fetch"]);
    assert!(ok, "stderr: {err}");
    assert!(
        err.contains("feature-x (1 unmerged commit)"),
        "stderr: {err}"
    );
    assert!(!err.contains("merged-x ("), "stderr: {err}");
}

/// A branch matching `git config gone.protect` (wildcards supported) is never a deletion
/// candidate — and the skip is announced, not silent.
#[test]
fn protected_branch_from_config_is_never_a_candidate() {
    let (_tmp, work) = repo_with_gone_branch();
    git(&work, &["config", "gone.protect", "feature-*"]);

    let (ok, _out, err) = gone_bin(&work, &["--yes", "--no-fetch"]);
    assert!(ok, "stderr: {err}");
    assert!(
        err.contains("Protected branches skipped: feature-x"),
        "stderr: {err}"
    );
    assert!(err.contains("No gone branches"), "stderr: {err}");
    assert!(branches(&work).iter().any(|b| b == "feature-x"));
}

#[test]
fn protect_flag_adds_to_config() {
    let (_tmp, work) = repo_with_gone_branch();

    let (ok, _out, err) = gone_bin(&work, &["--yes", "--no-fetch", "--protect", "feature-x"]);
    assert!(ok, "stderr: {err}");
    assert!(
        err.contains("Protected branches skipped: feature-x"),
        "stderr: {err}"
    );
    assert!(branches(&work).iter().any(|b| b == "feature-x"));
}

#[test]
fn list_does_not_delete() {
    let (_tmp, work) = repo_with_gone_branch();

    let (ok, out, _err) = gone_bin(&work, &["--list", "--no-fetch"]);
    assert!(ok);
    assert!(
        out.is_empty(),
        "list should print to stderr only; stdout: {out}"
    );
    assert!(branches(&work).iter().any(|b| b == "feature-x"));
}

#[test]
fn dry_run_does_not_delete() {
    let (_tmp, work) = repo_with_gone_branch();

    let (ok, _out, err) = gone_bin(&work, &["--dry-run", "--no-fetch"]);
    assert!(ok);
    assert!(err.contains("dry-run"));
    assert!(branches(&work).iter().any(|b| b == "feature-x"));
}

#[test]
fn no_gone_branches_reports_clean() {
    let tmp = TempDir::new().unwrap();
    let work = tmp.path().join("repo");
    git(tmp.path(), &["init", "-b", "main", &path(&work)]);
    fs::write(work.join("a.txt"), "a").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "init"]);

    let (ok, _out, err) = gone_bin(&work, &["--yes", "--no-fetch"]);
    assert!(ok);
    assert!(err.contains("No gone branches"));
}

#[test]
fn keeps_branch_with_live_upstream() {
    let (_tmp, work) = repo_with_gone_branch();

    git(&work, &["checkout", "-b", "feature-live"]);
    fs::write(work.join("live.txt"), "1").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "live"]);
    git(&work, &["push", "-u", "origin", "feature-live"]);
    git(&work, &["checkout", "main"]);

    let (ok, out, err) = gone_bin(&work, &["--yes", "--no-fetch"]);
    assert!(ok, "stderr: {err}");
    assert!(
        !out.contains("feature-live"),
        "should not touch live branch: {out}"
    );
    assert!(branches(&work).iter().any(|b| b == "feature-live"));
}

#[test]
fn refuses_without_tty_unless_yes() {
    let (_tmp, work) = repo_with_gone_branch();

    let out = Command::new(BIN)
        .arg("--no-fetch")
        .current_dir(&work)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(!out.status.success(), "should refuse non-tty without --yes");
    let code = out.status.code().unwrap();
    assert_eq!(code, 1);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--yes"));
    assert!(branches(&work).iter().any(|b| b == "feature-x"));
}

/// The default path (no `--no-fetch`) must run `git fetch --prune` itself: a branch deleted
/// on the remote elsewhere is invisible until the stale remote-tracking ref is pruned.
#[test]
fn default_fetches_and_prunes_before_detecting() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let bare = root.join("remote.git");
    let work = root.join("repo");

    git(root, &["init", "--bare", "-b", "main", &path(&bare)]);
    git(root, &["clone", &path(&bare), &path(&work)]);

    fs::write(work.join("a.txt"), "a").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "init"]);
    git(&work, &["push", "-u", "origin", "main"]);

    git(&work, &["checkout", "-b", "feature-x"]);
    fs::write(work.join("x.txt"), "x").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "x"]);
    git(&work, &["push", "-u", "origin", "feature-x"]);
    git(&work, &["checkout", "main"]);

    // Remote-side deletion performed elsewhere (e.g. a merged PR): the local
    // `refs/remotes/origin/feature-x` survives until something prunes it.
    git(&bare, &["update-ref", "-d", "refs/heads/feature-x"]);
    assert!(
        git(
            &work,
            &["for-each-ref", "--format=%(refname)", "refs/remotes/"],
        )
        .contains("origin/feature-x"),
        "precondition: stale tracking ref must still exist"
    );

    let (ok, out, err) = gone_bin(&work, &["--yes"]);
    assert!(ok, "stderr: {err}");
    assert!(err.contains("Fetching and pruning"), "stderr: {err}");
    assert!(out.contains("Deleted branch: feature-x"), "stdout: {out}");
    assert!(!branches(&work).iter().any(|b| b == "feature-x"));
}

#[test]
fn json_prints_a_document_when_nothing_is_gone() {
    let tmp = TempDir::new().unwrap();
    let work = tmp.path().join("repo");
    git(tmp.path(), &["init", "-b", "main", &path(&work)]);
    fs::write(work.join("a.txt"), "a").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "init"]);

    let (ok, out, _err) = gone_bin(&work, &["--json", "--no-fetch"]);
    assert!(ok);
    assert_eq!(out.trim(), "{\"gone\":[],\"skipped\":[]}");
}

#[test]
fn json_single_lists_gone_branches() {
    let (_tmp, work) = repo_with_gone_branch();

    let (ok, out, _err) = gone_bin(&work, &["--json", "--no-fetch"]);
    assert!(ok);
    assert_eq!(out.trim(), "{\"gone\":[\"feature-x\"],\"skipped\":[]}");
    assert!(branches(&work).iter().any(|b| b == "feature-x"));
}

#[test]
fn not_in_repo_errors() {
    let tmp = TempDir::new().unwrap();
    let (ok, _out, err) = gone_bin(tmp.path(), &["--yes", "--no-fetch"]);
    assert!(!ok);
    assert!(err.contains("not inside a Git working tree"));
}

/// `git rev-parse --is-inside-work-tree` prints `false` with exit status 0 in a bare
/// repository — the answer, not the exit status, must be what rejects it.
#[test]
fn a_bare_repository_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let bare = tmp.path().join("bare.git");
    git(tmp.path(), &["init", "--bare", "-b", "main", &path(&bare)]);

    let (ok, _out, err) = gone_bin(&bare, &["--yes", "--no-fetch"]);
    assert!(!ok, "a bare repository has no work tree; stderr: {err}");
    assert!(
        err.contains("not inside a Git working tree"),
        "stderr: {err}"
    );
}

// ---------- multi-repo mode (--recursive) ----------

/// Builds `<parent>/<name>` as a git repo whose `<feature>` branch is `[gone]`.
fn make_gone_repo(parent: &Path, name: &str, feature: &str) -> PathBuf {
    let bare = parent.join(format!("{name}.git"));
    let work = parent.join(name);
    git(parent, &["init", "--bare", "-b", "main", &path(&bare)]);
    git(parent, &["clone", &path(&bare), &path(&work)]);

    fs::write(work.join("a.txt"), "a").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "init"]);
    git(&work, &["push", "-u", "origin", "main"]);

    git(&work, &["checkout", "-b", feature]);
    fs::write(work.join("x.txt"), "x").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "x"]);
    git(&work, &["push", "-u", "origin", feature]);

    git(&work, &["checkout", "main"]);
    git(&work, &["push", "origin", "--delete", feature]);
    git(&work, &["fetch", "--prune"]);
    work
}

/// Builds a clean git repo (no gone branches) at `<parent>/<name>`.
fn make_clean_repo(parent: &Path, name: &str) -> PathBuf {
    let work = parent.join(name);
    git(parent, &["init", "-b", "main", &path(&work)]);
    fs::write(work.join("a.txt"), "a").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "init"]);
    work
}

/// A monorepo-style tree: two packages with gone branches, one clean, one non-repo dir.
fn setup_monorepo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_gone_repo(root, "pkg-a", "feat-a");
    make_gone_repo(root, "pkg-b", "feat-b");
    make_clean_repo(root, "pkg-clean");
    let empty = root.join("not-a-repo");
    fs::create_dir_all(&empty).unwrap();
    fs::write(empty.join("file.txt"), "x").unwrap();
    tmp
}

#[test]
fn recursive_finds_and_deletes_in_all_repos() {
    let tmp = setup_monorepo();
    let root = tmp.path();

    let (ok, out, err) = gone_bin(root, &["-r", "--yes", "--no-fetch"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains("Deleted pkg-a/feat-a"), "stdout: {out}");
    assert!(out.contains("Deleted pkg-b/feat-b"), "stdout: {out}");

    assert!(!branches(&root.join("pkg-a")).iter().any(|b| b == "feat-a"));
    assert!(!branches(&root.join("pkg-b")).iter().any(|b| b == "feat-b"));
}

#[test]
fn recursive_list_does_not_delete() {
    let tmp = setup_monorepo();
    let root = tmp.path();

    let (ok, _out, err) = gone_bin(root, &["-r", "--list", "--no-fetch"]);
    assert!(ok);
    assert!(err.contains("pkg-a") && err.contains("feat-a"));
    assert!(err.contains("pkg-b") && err.contains("feat-b"));
    assert!(branches(&root.join("pkg-a")).iter().any(|b| b == "feat-a"));
    assert!(branches(&root.join("pkg-b")).iter().any(|b| b == "feat-b"));
}

#[test]
fn recursive_root_scans_a_tree_other_than_the_current_directory() {
    let tmp = setup_monorepo();
    let root = tmp.path();
    let current = root.join("elsewhere");
    fs::create_dir_all(&current).unwrap();
    let root_arg = path(root);

    let (ok, _out, err) = gone_bin(
        &current,
        &["-r", "--root", &root_arg, "--list", "--no-fetch"],
    );

    assert!(ok, "stderr: {err}");
    assert!(
        err.contains("pkg-a") && err.contains("feat-a"),
        "stderr: {err}"
    );
    assert!(
        err.contains("pkg-b") && err.contains("feat-b"),
        "stderr: {err}"
    );
}

#[test]
fn recursive_json_output() {
    let tmp = setup_monorepo();
    let root = tmp.path();

    let (ok, out, _err) = gone_bin(root, &["-r", "--json", "--no-fetch"]);
    assert!(ok);
    assert!(out.contains("\"path\":\"pkg-a\""), "stdout: {out}");
    assert!(out.contains("feat-a"));
    assert!(out.contains("\"path\":\"pkg-b\""));
    assert!(out.contains("\"total_gone\":2"));
    assert!(branches(&root.join("pkg-a")).iter().any(|b| b == "feat-a"));
}

#[test]
fn json_include_reasons_reports_upstream_and_unmerged_commits() {
    let (_tmp, work) = repo_with_gone_branch();

    let (ok, out, err) = gone_bin(&work, &["--json", "--include-reasons", "--no-fetch"]);

    assert!(ok, "stderr: {err}");
    assert!(
        out.contains("\"reasons\":[{\"branch\":\"feature-x\""),
        "stdout: {out}"
    );
    assert!(
        out.contains("\"upstream\":\"refs/remotes/origin/feature-x\""),
        "stdout: {out}"
    );
    assert!(out.contains("\"unmerged_commits\":1"), "stdout: {out}");
}

/// A repository whose fetch fails (offline, dead remote) is still inspected, but the JSON
/// must say so: `"gone":[]` after a failed fetch is stale, not verified-clean.
#[test]
fn recursive_json_reports_a_failed_fetch() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let work = make_gone_repo(root, "pkg-a", "feat-a");
    git(
        &work,
        &[
            "remote",
            "set-url",
            "origin",
            &path(&root.join("no-such-remote.git")),
        ],
    );

    let (ok, out, err) = gone_bin(root, &["-r", "--json"]);
    assert!(
        ok,
        "a failed fetch is a warning, not a failure; stderr: {err}"
    );
    assert!(out.contains("\"fetch_error\":\""), "stdout: {out}");
    assert!(out.contains("\"error\":null"), "stdout: {out}");
    assert!(
        out.contains("feat-a"),
        "detection must still run on the stale refs; stdout: {out}"
    );
    assert!(err.contains("Warning:"), "stderr: {err}");
}

/// A failed refresh makes the tracking refs untrustworthy. They may still be useful in JSON
/// and list reports, but `--yes` must not delete an unpushed branch whose live upstream merely
/// lacks a local tracking ref.
#[test]
fn recursive_does_not_delete_after_a_failed_fetch() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let bare = root.join("remote.git");
    let work = root.join("repo");
    git(root, &["init", "--bare", "-b", "main", &path(&bare)]);
    git(root, &["clone", &path(&bare), &path(&work)]);
    git(&work, &["commit", "--allow-empty", "-m", "init"]);
    git(&work, &["push", "-u", "origin", "main"]);
    git(&work, &["checkout", "-b", "feature-live"]);
    git(&work, &["push", "-u", "origin", "feature-live"]);
    git(&work, &["commit", "--allow-empty", "-m", "local-only"]);
    git(&work, &["checkout", "main"]);
    git(
        &work,
        &["update-ref", "-d", "refs/remotes/origin/feature-live"],
    );
    git(
        &work,
        &[
            "remote",
            "set-url",
            "origin",
            &path(&root.join("no-such-remote.git")),
        ],
    );

    let (ok, out, err) = gone_bin(root, &["-r", "--yes"]);

    assert!(!ok, "a failed fetch must prevent deletion; stderr: {err}");
    assert!(out.is_empty(), "stdout: {out}");
    assert!(err.contains("could not be refreshed"), "stderr: {err}");
    assert!(branches(&work).iter().any(|b| b == "feature-live"));
    assert!(
        !git(&bare, &["show-ref", "--verify", "refs/heads/feature-live"]).is_empty(),
        "the upstream must remain live"
    );
}

#[test]
fn recursive_deletes_verified_repositories_and_summarizes_failed_refreshes() {
    let tmp = setup_monorepo();
    let root = tmp.path();
    git(
        &root.join("pkg-a"),
        &[
            "remote",
            "set-url",
            "origin",
            &path(&root.join("no-such-remote.git")),
        ],
    );

    let (ok, out, err) = gone_bin(root, &["-r", "--yes"]);

    assert!(!ok, "a failed refresh must produce exit 1; stderr: {err}");
    assert!(!out.contains("Deleted pkg-a/feat-a"), "stdout: {out}");
    assert!(out.contains("Deleted pkg-b/feat-b"), "stdout: {out}");
    assert!(branches(&root.join("pkg-a")).iter().any(|b| b == "feat-a"));
    assert!(!branches(&root.join("pkg-b")).iter().any(|b| b == "feat-b"));
    assert!(
        err.contains("Skipped deletion in 1 repository because refresh failed."),
        "stderr: {err}"
    );
}

/// Gitfiles are necessary for valid submodules and worktrees, but recursively discovered
/// metadata must not redirect the scan to a Git directory outside the requested root.
#[test]
fn recursive_rejects_a_gitfile_that_points_outside_the_scan_root() {
    let (tmp, victim) = repo_with_gone_branch();
    let root = tmp.path().join("scan");
    let attacker = root.join("attacker");
    fs::create_dir_all(&attacker).unwrap();
    fs::write(
        attacker.join(".git"),
        format!("gitdir: {}\n", victim.join(".git").display()),
    )
    .unwrap();

    let (ok, out, err) = gone_bin(&root, &["-r", "--yes", "--no-fetch"]);

    assert!(!ok, "unsafe Git metadata must fail the scan; stderr: {err}");
    assert!(out.is_empty(), "stdout: {out}");
    assert!(err.contains("outside scan root"), "stderr: {err}");
    assert!(branches(&victim).iter().any(|b| b == "feature-x"));
}

#[test]
fn recursive_allows_external_git_metadata_only_with_explicit_opt_in() {
    let (tmp, victim) = repo_with_gone_branch();
    let root = tmp.path().join("scan");
    let linked = root.join("linked-worktree");
    fs::create_dir_all(&linked).unwrap();
    fs::write(
        linked.join(".git"),
        format!("gitdir: {}\n", victim.join(".git").display()),
    )
    .unwrap();

    let (ok, out, err) = gone_bin(
        &root,
        &["-r", "--allow-external-git-dir", "--list", "--no-fetch"],
    );

    assert!(ok, "stderr: {err}");
    assert!(out.is_empty(), "stdout: {out}");
    assert!(err.contains("linked-worktree") && err.contains("feature-x"));
    assert!(branches(&victim).iter().any(|b| b == "feature-x"));
}

#[test]
fn recursive_exclude_skips_named_dir() {
    let tmp = setup_monorepo();
    let root = tmp.path();

    let (ok, _out, err) = gone_bin(root, &["-r", "--list", "--no-fetch", "--exclude", "pkg-a"]);
    assert!(ok);
    assert!(
        !err.contains("feat-a"),
        "pkg-a excluded but reported: {err}"
    );
    assert!(err.contains("feat-b"));
}

/// The fetch phase runs in parallel (`-j`); local bare remotes make it a real fetch.
#[test]
fn recursive_parallel_fetch_deletes_in_all_repos() {
    let tmp = setup_monorepo();
    let root = tmp.path();

    let (ok, out, err) = gone_bin(root, &["-r", "--yes", "-j", "4"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains("Deleted pkg-a/feat-a"), "stdout: {out}");
    assert!(out.contains("Deleted pkg-b/feat-b"), "stdout: {out}");
    assert!(
        err.contains("Recover a deleted branch with: git branch"),
        "stderr: {err}"
    );
}

/// `git config gone.exclude` sits between the built-in default and the `--exclude` flag;
/// the flag replaces it wholesale.
#[test]
fn recursive_exclude_from_git_config() {
    let tmp = setup_monorepo();
    let root = tmp.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "gone.exclude", "pkg-a"]);

    let (ok, _out, err) = gone_bin(root, &["-r", "--list", "--no-fetch"]);
    assert!(ok, "stderr: {err}");
    assert!(!err.contains("feat-a"), "pkg-a excluded via config: {err}");
    assert!(err.contains("feat-b"), "stderr: {err}");

    let (ok, _out, err) = gone_bin(root, &["-r", "--list", "--no-fetch", "--exclude", "pkg-b"]);
    assert!(ok, "stderr: {err}");
    assert!(
        err.contains("feat-a"),
        "the flag replaces the config: {err}"
    );
    assert!(!err.contains("feat-b"), "stderr: {err}");
}

#[test]
fn recursive_depth_limits_descent() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_gone_repo(root, "pkg-top", "feat-top");
    let nested = root.join("nested");
    fs::create_dir_all(&nested).unwrap();
    make_gone_repo(&nested, "pkg-deep", "feat-deep");

    let (ok, _out, err) = gone_bin(root, &["-r", "--list", "--no-fetch", "--depth", "1"]);
    assert!(ok);
    assert!(err.contains("feat-top"));
    assert!(
        !err.contains("feat-deep"),
        "depth 1 should not reach nested/pkg-deep: {err}"
    );
}

#[test]
fn recursive_no_repos_found() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("file.txt"), "x").unwrap();

    let (ok, _out, err) = gone_bin(tmp.path(), &["-r", "--list", "--no-fetch"]);
    assert!(ok);
    assert!(err.contains("No Git repositories"));
}

#[test]
fn recursive_json_prints_a_document_when_no_repos_found() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("file.txt"), "x").unwrap();

    let (ok, out, _err) = gone_bin(tmp.path(), &["-r", "--json", "--no-fetch"]);
    assert!(ok);
    assert!(out.contains("\"repositories\":[]"), "stdout: {out}");
    assert!(out.contains("\"total_gone\":0"), "stdout: {out}");
}

/// Invalid Git metadata must fail the scan, never be passed through to Git as a repository.
#[test]
fn recursive_reports_invalid_git_metadata_as_a_scan_error() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_gone_repo(root, "pkg-a", "feat-a");

    // A malformed gitfile is not a repository. Passing it to Git could allow a crafted
    // gitfile to redirect the recursive scan to metadata outside the requested root.
    let broken = root.join("pkg-broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join(".git"), "gitdir: ./nonexistent\n").unwrap();

    // The healthy repo remains in the document, but the invalid metadata causes a non-zero
    // exit code so `git gone -r --json && …` cannot pass over an incomplete scan.
    let (ok, out, err) = gone_bin(root, &["-r", "--json", "--no-fetch"]);
    assert!(!ok, "invalid metadata must not exit 0; stderr: {err}");
    assert!(!out.contains("\"path\":\"pkg-broken\""), "stdout: {out}");
    assert!(out.contains("\"error\":null"), "stdout: {out}");
    assert!(
        err.contains("could not resolve Git metadata"),
        "stderr: {err}"
    );

    let (ok, _out, err_list) = gone_bin(root, &["-r", "--list", "--no-fetch"]);
    assert!(!ok, "scan failure must not exit 0; stderr: {err_list}");
    assert!(
        err_list.contains("could not resolve Git metadata"),
        "stderr: {err_list}"
    );
}

#[test]
fn depth_without_recursive_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let (ok, _out, err) = gone_bin(tmp.path(), &["--depth", "1"]);
    assert!(!ok);
    assert!(
        err.contains("--recursive"),
        "clap should require -r; stderr: {err}"
    );
}

// ---------- branch names and checked-out branches ----------

/// `git branch` refuses to create a ref whose name starts with `-`, but `git update-ref`
/// and `git fetch` can produce one. Deletion must pass it after `--` instead of letting
/// Git parse it as an option.
#[test]
fn a_branch_whose_name_starts_with_a_dash_is_deleted() {
    let (_tmp, work) = repo_with_gone_branch();

    git(&work, &["update-ref", "refs/heads/-x", "HEAD"]);
    git(&work, &["config", "branch.-x.remote", "origin"]);
    git(&work, &["config", "branch.-x.merge", "refs/heads/-x"]);
    assert!(branches(&work).iter().any(|b| b == "-x"));

    let (ok, out, err) = gone_bin(&work, &["--yes", "--no-fetch"]);
    assert!(ok, "stderr: {err}");
    // The SHA lookup must survive the leading dash too — it queries `refs/heads/-x`.
    assert!(out.contains("Deleted branch: -x (was "), "stdout: {out}");
    assert!(!branches(&work).iter().any(|b| b == "-x"), "stderr: {err}");
}

#[test]
fn safe_mode_skips_an_unmerged_branch_before_deletion() {
    let (_tmp, work) = repo_with_gone_branch();

    let (ok, out, err) = gone_bin(&work, &["--yes", "--safe", "--no-fetch"]);

    assert!(ok, "safe mode must skip unmerged work; stderr: {err}");
    assert!(out.is_empty(), "stdout: {out}");
    assert!(err.contains("Safe mode skipped 1 branch(es) not merged into HEAD."));
    assert!(branches(&work).iter().any(|b| b == "feature-x"));
}

#[test]
fn safe_mode_deletes_a_merged_branch() {
    let (_tmp, work) = repo_with_gone_branch();
    git(&work, &["branch", "merged-x", "main"]);
    git(&work, &["config", "branch.merged-x.remote", "origin"]);
    git(
        &work,
        &["config", "branch.merged-x.merge", "refs/heads/merged-x"],
    );
    git(&work, &["config", "gone.protect", "feature-x"]);

    let (ok, out, err) = gone_bin(&work, &["--yes", "--safe", "--no-fetch"]);

    assert!(ok, "stderr: {err}");
    assert!(out.contains("Deleted branch: merged-x"), "stdout: {out}");
    assert!(!branches(&work).iter().any(|b| b == "merged-x"));
    assert!(branches(&work).iter().any(|b| b == "feature-x"));
}

#[test]
fn squash_safe_mode_deletes_an_exact_tree_match() {
    let (_tmp, work) = repo_with_gone_branch();
    add_squash_merged_gone_branch(&work);

    let (ok, out, err) = gone_bin(&work, &["--yes", "--squash-safe", "--no-fetch"]);

    assert!(
        ok,
        "squash-safe mode must delete exact tree matches; stderr: {err}"
    );
    assert!(out.contains("Deleted branch: squash-x"), "stdout: {out}");
    assert!(!branches(&work).iter().any(|b| b == "squash-x"));
    assert!(branches(&work).iter().any(|b| b == "feature-x"));
    assert!(err.contains("Squash-safe mode skipped 1 branch(es)"));
}

#[test]
fn recursive_safe_mode_skips_unmerged_branches_before_confirmation() {
    let tmp = setup_monorepo();
    let root = tmp.path();

    let (ok, out, err) = gone_bin(root, &["-r", "--yes", "--safe", "--no-fetch"]);

    assert!(ok, "safe mode must skip unmerged work; stderr: {err}");
    assert!(out.is_empty(), "stdout: {out}");
    assert!(err.contains("Safe mode skipped 2 branch(es) not merged into HEAD."));
    assert!(branches(&root.join("pkg-a")).iter().any(|b| b == "feat-a"));
    assert!(branches(&root.join("pkg-b")).iter().any(|b| b == "feat-b"));
}

#[test]
fn recursive_squash_safe_mode_deletes_an_exact_tree_match() {
    let (tmp, work) = repo_with_gone_branch();
    add_squash_merged_gone_branch(&work);

    let (ok, out, err) = gone_bin(tmp.path(), &["-r", "--yes", "--squash-safe", "--no-fetch"]);

    assert!(
        ok,
        "squash-safe mode must delete exact tree matches; stderr: {err}"
    );
    assert!(out.contains("Deleted repo/squash-x"), "stdout: {out}");
    assert!(!branches(&work).iter().any(|b| b == "squash-x"));
    assert!(branches(&work).iter().any(|b| b == "feature-x"));
}

/// A gone branch checked out in another worktree cannot be deleted by Git, so it must not
/// be offered as a candidate: otherwise every run ends in a guaranteed failure and exit 1.
#[test]
fn a_branch_checked_out_in_another_worktree_is_not_a_candidate() {
    let (_tmp, work) = repo_with_gone_branch();
    let elsewhere = work.parent().unwrap().join("wt");
    git(&work, &["worktree", "add", &path(&elsewhere), "feature-x"]);

    let (ok, out, err) = gone_bin(&work, &["--json", "--no-fetch"]);
    assert!(ok, "stderr: {err}");
    assert_eq!(out.trim(), "{\"gone\":[],\"skipped\":[]}");

    let (ok, _out, err) = gone_bin(&work, &["--yes", "--no-fetch"]);
    assert!(ok, "stderr: {err}");
    assert!(branches(&work).iter().any(|b| b == "feature-x"));
}

/// Detached HEAD has no current branch; detection must still work rather than error out.
#[test]
fn detached_head_still_detects_gone_branches() {
    let (_tmp, work) = repo_with_gone_branch();
    git(&work, &["checkout", "--detach"]);

    let (ok, out, err) = gone_bin(&work, &["--json", "--no-fetch"]);
    assert!(ok, "stderr: {err}");
    assert_eq!(out.trim(), "{\"gone\":[\"feature-x\"],\"skipped\":[]}");
}

/// A branch name with non-UTF-8 bytes cannot be round-tripped back to Git: decoding it
/// lossily would query a mangled ref, get "does not resolve", and report a branch with a
/// live upstream as gone. It must be skipped with a warning instead.
#[cfg(target_os = "linux")]
#[test]
fn a_non_utf8_branch_name_is_skipped_not_reported_as_gone() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let (_tmp, work) = repo_with_gone_branch();
    let head = git(&work, &["rev-parse", "HEAD"]);

    for refname in [
        &b"refs/heads/bad\xffbranch"[..],
        &b"refs/remotes/origin/bad\xffbranch"[..],
    ] {
        let out = Command::new("git")
            .args([OsStr::new("update-ref"), OsStr::from_bytes(refname)])
            .arg(&head)
            .current_dir(&work)
            .output()
            .unwrap();
        assert!(out.status.success(), "update-ref: {out:?}");
    }
    // The upstream ref above exists, so this branch is alive — only the lossy decoding
    // could make it look gone.
    let out = Command::new("git")
        .arg("config")
        .arg(OsStr::from_bytes(b"branch.bad\xffbranch.remote"))
        .arg("origin")
        .current_dir(&work)
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = Command::new("git")
        .arg("config")
        .arg(OsStr::from_bytes(b"branch.bad\xffbranch.merge"))
        .arg(OsStr::from_bytes(b"refs/heads/bad\xffbranch"))
        .current_dir(&work)
        .output()
        .unwrap();
    assert!(out.status.success());

    let (ok, out, err) = gone_bin(&work, &["--json", "--no-fetch"]);
    assert!(ok, "stderr: {err}");
    assert_eq!(
        out.trim(),
        "{\"gone\":[\"feature-x\"],\"skipped\":[\"bad\u{fffd}branch\"]}",
        "a live branch must not be reported as gone, and the skipped name must reach the JSON consumer"
    );
    assert!(
        err.contains("unparsable name"),
        "the skipped branch must be announced; stderr: {err}"
    );
}

/// A directory that cannot be read hides every repository underneath it — that is a failed
/// scan, not an empty one.
#[cfg(unix)]
#[test]
fn recursive_reports_an_unreadable_directory() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_gone_repo(root, "pkg-a", "feat-a");
    let locked = root.join("locked");
    fs::create_dir_all(locked.join("pkg/.git")).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let (ok, _out, err) = gone_bin(root, &["-r", "--list", "--no-fetch"]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    // Running as root defeats the permission bits and there is nothing to observe.
    if err.contains("could not read") {
        assert!(
            !ok,
            "an unreadable directory must not exit 0; stderr: {err}"
        );
    }
}
