use std::io::{self, IsTerminal, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;

use crate::cli::{Cli, DEFAULT_EXCLUDE};
use crate::git::{self, DeleteMode, Detection, Prompt};
use crate::json;
use crate::report::{self, RepoReport};
use crate::scan::{find_repos, rel_path};

// ---------- single-repo mode (default) ----------

pub(crate) fn single(work: &Path, cli: &Cli) -> Result<u8> {
    git::ensure_in_repo(work)?;

    if !cli.no_fetch {
        if !cli.json {
            eprintln!("Fetching and pruning remote-tracking refs...");
        }
        git::fetch_prune(work, Prompt::Allow)?;
    }

    let mut detection = git::collect_gone(work)?;
    let protected = apply_protection(work, cli, &mut detection);
    note_protected(&protected, None);
    warn_skipped(&detection, None);

    if cli.json {
        let document = if cli.include_reasons {
            json::single_document_with_reasons(work, &detection)
        } else {
            json::single_document(&detection.gone, &detection.skipped)
        };
        println!("{document}");

        return Ok(0);
    }

    if cli.safe {
        let safe = retain_merged_branches(work, &mut detection);
        note_safe_skipped(&safe);
    }

    let gone = detection.gone;
    if gone.is_empty() {
        eprintln!("No gone branches to delete.");

        return Ok(0);
    }

    eprintln!("Gone branches ({}):", gone.len());
    for b in &gone {
        eprintln!("  {b}{}", unmerged_suffix(work, b));
    }

    if cli.is_report_only() {
        if cli.dry_run {
            eprintln!("(dry-run: no branches were deleted)");
        }

        return Ok(0);
    }

    let word = if gone.len() == 1 {
        "branch"
    } else {
        "branches"
    };
    if !confirm(cli, &format!("Delete these {} {word}?", gone.len()))? {
        return Ok(1);
    }

    Ok(delete_in_repo(work, &gone, delete_mode(cli)))
}

// ---------- multi-repo mode (--recursive) ----------

pub(crate) fn multi(root: &Path, cli: &Cli) -> Result<u8> {
    let scan = find_repos(
        root,
        cli.depth,
        &exclude_list(cli, root),
        cli.allow_external_git_dir,
    );
    for e in &scan.errors {
        eprintln!("Warning: {e}");
    }

    if scan.repos.is_empty() {
        if cli.json {
            println!("{}", json::multi_document(root, &[], 0));
        } else {
            eprintln!("No Git repositories found under {}.", root.display());
        }

        return Ok(u8::from(!scan.errors.is_empty()));
    }

    if !cli.json {
        eprintln!("Scanning {} repositories...", scan.repos.len());
    }

    let mut report = inspect_repos(&scan.repos, root, cli);
    if cli.safe && !cli.json {
        let safe = retain_merged_report_branches(&mut report);
        note_safe_skipped(&safe);
    }
    let destructive = !cli.json && !cli.is_report_only();
    let total = if destructive {
        report::total_deletable_gone(&report)
    } else {
        report::total_gone(&report)
    };
    let with_gone = if destructive {
        report
            .iter()
            .filter(|r| !r.deletable_gone().is_empty())
            .count()
    } else {
        report::repos_with_gone(&report)
    };
    // A directory that could not be read and a repository that could not be inspected both
    // mean the report is incomplete — neither may be presented as a clean result.
    let fetch_failures = if destructive {
        report::failed_fetches(&report)
    } else {
        0
    };
    let failures = scan.errors.len() + report::failed_inspections(&report) + fetch_failures;

    if cli.json {
        let document = if cli.include_reasons {
            json::multi_document_with_reasons(root, &report, total)
        } else {
            json::multi_document(root, &report, total)
        };
        println!("{document}");

        return Ok(u8::from(failures > 0));
    }

    for r in &report {
        if let Some(err) = r.error() {
            eprintln!(
                "Warning: could not inspect {}: {err}",
                rel_path(&r.path, root)
            );
        }
    }

    if total == 0 {
        if destructive && fetch_failures > 0 {
            eprintln!(
                "No branches are eligible for deletion because {} repositor{} could not be refreshed.",
                fetch_failures,
                plural_y(fetch_failures)
            );
        } else {
            eprintln!("No gone branches across {} repositories.", report.len());
        }

        return Ok(u8::from(failures > 0));
    }

    print_gone_tree(&report, root, total, with_gone, destructive);
    note_fetch_failures(fetch_failures);

    if cli.is_report_only() {
        if cli.dry_run {
            eprintln!("(dry-run: no branches were deleted)");
        }

        return Ok(u8::from(failures > 0));
    }

    let prompt = format!(
        "Delete {total} branch(es) across {with_gone} repositor{}?",
        plural_y(with_gone)
    );
    if !confirm(cli, &prompt)? {
        return Ok(1);
    }

    let (deleted, failed_deletions) = delete_all(&report, root, delete_mode(cli));
    if deleted > 0 {
        eprintln!("{RESTORE_HINT}");
    }
    let failed = failures + failed_deletions;
    if failed > 0 {
        eprintln!("{failed} repository/branch operation(s) failed.");
        Ok(1)
    } else {
        eprintln!("Done.");
        Ok(0)
    }
}

/// Fetches (unless `--no-fetch`) and detects in every repository. The fetch phase runs in
/// parallel (`--jobs`) — it is network-bound and dominates the wall clock; detection is
/// local and stays sequential. Fetch errors are per-repo and non-fatal (no remote,
/// offline) but recorded in the report; detection errors are never treated as clean.
fn inspect_repos(repos: &[PathBuf], root: &Path, cli: &Cli) -> Vec<RepoReport> {
    let fetch_errors: Vec<Option<String>> = if cli.no_fetch {
        repos.iter().map(|_| None).collect()
    } else {
        fetch_all(repos, root, effective_jobs(cli))
    };

    let mut report = Vec::with_capacity(repos.len());
    for (repo, fetch_error) in repos.iter().zip(fetch_errors) {
        if let Some(e) = &fetch_error {
            eprintln!("Warning: {e}");
        }
        let mut result = git::collect_gone(repo).map_err(|e| format!("{e:#}"));
        if let Ok(detection) = &mut result {
            let rel = rel_path(repo, root);
            let protected = apply_protection(repo, cli, detection);
            note_protected(&protected, Some(&rel));
            warn_skipped(detection, Some(&rel));
        }
        report.push(RepoReport {
            path: repo.clone(),
            result,
            fetch_error,
        });
    }
    report
}

/// Runs `git fetch --prune` in every repository, `jobs` at a time. Results are indexed, so
/// the report order stays deterministic regardless of completion order. A progress line is
/// shown only when stderr is a terminal — a long batch otherwise looks hung.
fn fetch_all(repos: &[PathBuf], root: &Path, jobs: usize) -> Vec<Option<String>> {
    let results: Vec<Mutex<Option<String>>> = repos.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let progress = io::stderr().is_terminal();

    std::thread::scope(|s| {
        for _ in 0..jobs.min(repos.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(repo) = repos.get(i) else { break };
                    let err = git::fetch_prune(repo, Prompt::Deny)
                        .err()
                        .map(|e| format!("{e:#}"));
                    *results[i].lock().unwrap() = err;
                    let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if progress {
                        // One write per update: `eprint!` locks stderr per call, so lines
                        // from parallel workers cannot interleave mid-line.
                        eprint!(
                            "\r\x1b[2K[{finished}/{}] {}",
                            repos.len(),
                            rel_path(repo, root)
                        );
                    }
                }
            });
        }
    });
    if progress {
        eprint!("\r\x1b[2K");
    }

    results
        .into_iter()
        .map(|m| m.into_inner().unwrap())
        .collect()
}

fn effective_jobs(cli: &Cli) -> usize {
    cli.jobs.map_or_else(
        || {
            std::thread::available_parallelism()
                .map_or(1, NonZeroUsize::get)
                .min(8)
        },
        NonZeroUsize::get,
    )
}

/// Directory names to skip during the scan: `--exclude` wins, then
/// `git config gone.exclude`, then the built-in default — each level replaces
/// the one below wholesale rather than adding to it.
fn exclude_list(cli: &Cli, root: &Path) -> Vec<String> {
    if let Some(flag) = &cli.exclude {
        return flag.clone();
    }
    let config = git::config_values(root, "gone.exclude");
    if config.is_empty() {
        DEFAULT_EXCLUDE.iter().map(|s| (*s).to_string()).collect()
    } else {
        config
    }
}

/// Removes branches matching `git config gone.protect` + `--protect` patterns from the
/// candidates and returns their names — they are announced, not silently dropped.
fn apply_protection(work: &Path, cli: &Cli, detection: &mut Detection) -> Vec<String> {
    let mut patterns = git::config_values(work, "gone.protect");
    if let Some(flag) = &cli.protect {
        patterns.extend(flag.iter().cloned());
    }
    if patterns.is_empty() {
        return Vec::new();
    }

    let (protected, kept) = std::mem::take(&mut detection.gone)
        .into_iter()
        .partition(|b| patterns.iter().any(|p| wildcard_match(p, b)));
    detection.gone = kept;
    detection
        .upstreams
        .retain(|branch, _| detection.gone.contains(branch));
    protected
}

fn note_protected(protected: &[String], repo: Option<&str>) {
    if protected.is_empty() {
        return;
    }
    let names = protected.join(", ");
    match repo {
        Some(rel) => eprintln!("Protected branches skipped in {rel}: {names}"),
        None => eprintln!("Protected branches skipped: {names}"),
    }
}

struct SafeFilter {
    unmerged: usize,
    errors: Vec<String>,
}

fn retain_merged_report_branches(report: &mut [RepoReport]) -> SafeFilter {
    let mut safe = SafeFilter {
        unmerged: 0,
        errors: Vec::new(),
    };
    for repo in report {
        if repo.fetch_error.is_some() {
            continue;
        }
        let Some(detection) = repo.result.as_mut().ok() else {
            continue;
        };
        let result = retain_merged_branches(&repo.path, detection);
        safe.unmerged += result.unmerged;
        safe.errors.extend(result.errors);
    }

    safe
}

fn retain_merged_branches(work: &Path, detection: &mut Detection) -> SafeFilter {
    let mut safe = SafeFilter {
        unmerged: 0,
        errors: Vec::new(),
    };
    let mut kept = Vec::with_capacity(detection.gone.len());
    for branch in std::mem::take(&mut detection.gone) {
        match git::is_merged_into_head(work, &branch) {
            Ok(true) => kept.push(branch),
            Ok(false) => safe.unmerged += 1,
            Err(e) => {
                safe.errors.push(format!(
                    "could not check whether {branch} is merged in {}: {e:#}",
                    work.display()
                ));
                safe.unmerged += 1;
            }
        }
    }
    detection.gone = kept;
    detection
        .upstreams
        .retain(|branch, _| detection.gone.contains(branch));

    safe
}

fn note_safe_skipped(safe: &SafeFilter) {
    if safe.unmerged > 0 {
        eprintln!(
            "Safe mode skipped {} branch(es) not merged into HEAD.",
            safe.unmerged
        );
    }
    for error in &safe.errors {
        eprintln!("Warning: {error}");
    }
}

fn note_fetch_failures(fetch_failures: usize) {
    if fetch_failures > 0 {
        eprintln!(
            "Skipped deletion in {fetch_failures} repositor{} because refresh failed.",
            plural_y(fetch_failures)
        );
    }
}

/// Anchored match where `*` stands for any run of characters — the only pattern syntax
/// `gone.protect` supports.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == name;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !name.starts_with(first) || name.len() < first.len() + last.len() || !name.ends_with(last) {
        return false;
    }
    let mut pos = first.len();
    let end = name.len() - last.len();
    for mid in &parts[1..parts.len() - 1] {
        if mid.is_empty() {
            continue;
        }
        match name[pos..end].find(mid) {
            Some(i) => pos += i + mid.len(),
            None => return false,
        }
    }

    true
}

/// Branches whose name could not be parsed are never deletion candidates, but they are
/// always announced: silently omitting a branch is the failure mode this tool avoids.
fn warn_skipped(detection: &Detection, repo: Option<&str>) {
    for name in &detection.skipped {
        match repo {
            Some(rel) => {
                eprintln!("Warning: skipping branch with an unparsable name in {rel}: {name}");
            }
            None => eprintln!("Warning: skipping branch with an unparsable name: {name}"),
        }
    }
}

fn print_gone_tree(
    report: &[RepoReport],
    root: &Path,
    total: usize,
    with_gone: usize,
    verified_only: bool,
) {
    for r in report {
        let gone = if verified_only {
            r.deletable_gone()
        } else {
            r.gone()
        };
        if gone.is_empty() {
            continue;
        }
        eprintln!();
        eprintln!("{}:", rel_path(&r.path, root));
        for b in gone {
            eprintln!("  {b}{}", unmerged_suffix(&r.path, b));
        }
    }

    eprintln!();
    eprintln!(
        "{total} gone branch(es) across {with_gone} repositor{} (of {} scanned).",
        plural_y(with_gone),
        report.len()
    );
}

/// ` (N unmerged commits)` — commits no other ref can reach, i.e. what the forced delete
/// would actually lose (reflog aside). Empty for a merged branch or when the count could
/// not be computed: the suffix is a best-effort warning, not a promise.
fn unmerged_suffix(work: &Path, branch: &str) -> String {
    match git::unmerged_count(work, branch) {
        Some(n) if n > 0 => {
            let word = if n == 1 { "commit" } else { "commits" };

            format!(" ({n} unmerged {word})")
        }
        _ => String::new(),
    }
}

const RESTORE_HINT: &str = "Recover a deleted branch with: git branch <name> <sha>";

/// Deletes every detected branch; returns `(deleted, failed)` counts.
fn delete_all(report: &[RepoReport], root: &Path, mode: DeleteMode) -> (usize, usize) {
    let mut deleted = 0usize;
    let mut failed = 0usize;
    for r in report {
        let rel = rel_path(&r.path, root);
        for branch in r.deletable_gone() {
            let was = was_suffix(&r.path, branch);
            match git::delete_branch(&r.path, branch, mode) {
                Ok(()) => {
                    println!("Deleted {rel}/{branch}{was}");
                    deleted += 1;
                }
                Err(e) => {
                    eprintln!("Failed to delete {rel}/{branch}: {e}");
                    failed += 1;
                }
            }
        }
    }
    (deleted, failed)
}

/// Deletes every branch in `branches`; returns the process exit code.
fn delete_in_repo(work: &Path, branches: &[String], mode: DeleteMode) -> u8 {
    let mut deleted = 0usize;
    let mut failed = 0usize;
    for branch in branches {
        let was = was_suffix(work, branch);
        match git::delete_branch(work, branch, mode) {
            Ok(()) => {
                println!("Deleted branch: {branch}{was}");
                deleted += 1;
            }
            Err(e) => {
                eprintln!("Failed to delete {branch}: {e}");
                failed += 1;
            }
        }
    }
    if deleted > 0 {
        eprintln!("{RESTORE_HINT}");
    }
    if failed > 0 {
        eprintln!("{failed} branch(es) could not be deleted.");
        1
    } else {
        eprintln!("Done.");
        0
    }
}

const fn delete_mode(cli: &Cli) -> DeleteMode {
    if cli.safe {
        DeleteMode::Safe
    } else {
        DeleteMode::Force
    }
}

/// ` (was <short-sha>)`, so a forced delete stays recoverable from the reflog. Read before
/// the deletion rather than parsed out of `git branch -D` output, which Git translates.
fn was_suffix(work: &Path, branch: &str) -> String {
    git::short_sha(work, branch).map_or_else(String::new, |sha| format!(" (was {sha})"))
}

/// Prompt the user for confirmation unless `--yes`. Returns `true` if confirmed.
fn confirm(cli: &Cli, question: &str) -> Result<bool> {
    if cli.yes {
        return Ok(true);
    }
    if !io::stdin().is_terminal() {
        eprintln!("Refusing to delete without a TTY; pass --yes to skip the confirmation prompt.");

        return Ok(false);
    }
    eprint!("{question} (y/N) ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim().to_lowercase();
    if answer == "y" || answer == "yes" {
        Ok(true)
    } else {
        eprintln!("Aborted.");

        Ok(false)
    }
}

const fn plural_y(n: usize) -> &'static str {
    if n == 1 { "y" } else { "ies" }
}

#[cfg(test)]
mod tests {
    use super::*;

    use clap::Parser;

    #[test]
    fn plural_y_switches_on_one() {
        assert_eq!(plural_y(1), "y");
        assert_eq!(plural_y(0), "ies");
        assert_eq!(plural_y(2), "ies");
    }

    #[test]
    fn was_suffix_is_empty_for_an_unresolvable_branch() {
        let tmp = tempfile::TempDir::new().unwrap();

        assert_eq!(was_suffix(tmp.path(), "no-such-branch"), "");
    }

    #[test]
    fn wildcard_match_is_anchored() {
        assert!(wildcard_match("main", "main"));
        assert!(!wildcard_match("main", "main-2"));
        assert!(!wildcard_match("main", "old-main"));
    }

    #[test]
    fn wildcard_star_spans_any_run_including_slashes() {
        assert!(wildcard_match("release/*", "release/1.0"));
        assert!(wildcard_match("release/*", "release/1.0/hotfix"));
        assert!(!wildcard_match("release/*", "release"));
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("*-wip", "feat/x-wip"));
        assert!(!wildcard_match("*-wip", "feat/x-wip2"));
        assert!(wildcard_match("a*b*c", "a-x-b-y-c"));
        assert!(!wildcard_match("a*b*c", "a-x-c-y-b"));
        // Overlap: the prefix and the suffix must not reuse the same characters.
        assert!(!wildcard_match("aa*aa", "aaa"));
        assert!(wildcard_match("aa*aa", "aaaa"));
    }

    #[test]
    fn wildcard_match_handles_multibyte_names() {
        assert!(wildcard_match("ветка/*", "ветка/раз"));
        assert!(!wildcard_match("ветка/*", "вет"));
    }

    #[test]
    fn exclude_flag_wins_over_defaults() {
        let tmp = tempfile::TempDir::new().unwrap();
        let with_flag = Cli::parse_from(["git-gone", "-r", "--exclude", "a,b"]);
        let without = Cli::parse_from(["git-gone", "-r"]);

        assert_eq!(
            exclude_list(&with_flag, tmp.path()),
            vec!["a".to_string(), "b".to_string()]
        );
        // No flag and (normally) no `gone.exclude` config in a fresh temp dir → defaults.
        // If the developer's own global config sets gone.exclude, that layer wins — which
        // is exactly the documented precedence, so only the flag case is asserted exactly.
        assert!(!exclude_list(&without, tmp.path()).is_empty());
    }

    #[test]
    fn apply_protection_partitions_and_reports() {
        let cli = Cli::parse_from(["git-gone", "--protect", "keep,rel/*"]);
        let mut detection = Detection {
            gone: vec![
                "drop".to_string(),
                "keep".to_string(),
                "rel/1.0".to_string(),
            ],
            skipped: Vec::new(),
            ..Detection::default()
        };
        let tmp = tempfile::TempDir::new().unwrap();

        let protected = apply_protection(tmp.path(), &cli, &mut detection);

        assert_eq!(detection.gone, vec!["drop".to_string()]);
        assert_eq!(protected, vec!["keep".to_string(), "rel/1.0".to_string()]);
    }
}
