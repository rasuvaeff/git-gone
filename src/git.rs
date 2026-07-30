use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

const GIT: &str = "git";

/// Whether `git` may ask the terminal for credentials. Denied in multi-repo mode: one
/// repository with a credentialed remote would otherwise block the whole batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Prompt {
    Allow,
    Deny,
}

/// Whether deletion may force-drop unmerged commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteMode {
    Force,
    Safe,
}

/// Detection result for one repository. `skipped` carries branches whose name could not
/// be parsed (non-UTF-8 bytes, or an embedded TAB): they are never deletion candidates
/// and are always reported as warnings — never silently dropped, never reported as gone.
#[derive(Debug, Default)]
pub(crate) struct Detection {
    pub(crate) gone: Vec<String>,
    pub(crate) skipped: Vec<String>,
    pub(crate) upstreams: BTreeMap<String, String>,
}

impl Detection {
    pub(crate) fn upstream(&self, branch: &str) -> Option<&str> {
        self.upstreams.get(branch).map(String::as_str)
    }
}

/// Detects gone branches in `work`, excluding branches checked out in any worktree.
pub(crate) fn collect_gone(work: &Path) -> Result<Detection> {
    let checked_out = checked_out_branches(work);
    let mut detection = gone_branches(work)?;
    detection.gone.retain(|b| !checked_out.contains(b));
    detection
        .upstreams
        .retain(|branch, _| detection.gone.contains(branch));
    detection.gone.sort();
    detection.gone.dedup();
    detection.skipped.sort();
    detection.skipped.dedup();

    Ok(detection)
}

pub(crate) fn ensure_in_repo(work: &Path) -> Result<()> {
    // The printed answer is what decides: `--is-inside-work-tree` prints `false` with
    // exit status 0 in a bare repository, so the exit status alone accepts bare repos.
    if git_stdout(work, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        bail!("not inside a Git working tree: {}", work.display());
    }
    Ok(())
}

pub(crate) fn fetch_prune(work: &Path, prompt: Prompt) -> Result<()> {
    let mut cmd = Command::new(GIT);
    cmd.args(["fetch", "--prune"])
        .current_dir(work)
        .stdout(Stdio::null());
    match prompt {
        // Interactive single-repo mode: git's progress and credential prompts pass through.
        Prompt::Allow => {
            cmd.stderr(Stdio::inherit());
            let status = cmd.status().with_context(|| {
                format!("failed to run `git fetch --prune` in {}", work.display())
            })?;
            if !status.success() {
                bail!(
                    "`git fetch --prune` exited with status {status} in {}",
                    work.display()
                );
            }
        }
        // Batch mode: fetches run in parallel, so stderr is captured instead of letting
        // several git processes interleave on the terminal; it becomes the error message.
        Prompt::Deny => {
            cmd.env("GIT_TERMINAL_PROMPT", "0");
            let out = cmd.output().with_context(|| {
                format!("failed to run `git fetch --prune` in {}", work.display())
            })?;
            if !out.status.success() {
                let detail = String::from_utf8_lossy(&out.stderr);
                let detail = detail.trim().lines().last().unwrap_or_default().to_string();
                bail!(
                    "`git fetch --prune` exited with status {} in {}: {detail}",
                    out.status,
                    work.display()
                );
            }
        }
    }
    Ok(())
}

/// `--` separates the branch name from options: a ref whose name starts with `-` cannot be
/// created by `git branch`, but `git update-ref` and `git fetch` can produce one.
pub(crate) fn delete_branch(work: &Path, name: &str, mode: DeleteMode) -> Result<()> {
    let flag = match mode {
        DeleteMode::Force => "-D",
        DeleteMode::Safe => "-d",
    };
    let status = Command::new(GIT)
        .args(["branch", flag, "--", name])
        .current_dir(work)
        // Git's own "Deleted branch …" line is translated; the caller prints an
        // untranslated message carrying the same short SHA.
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to run `git branch {flag} {name}`"))?;
    if !status.success() {
        bail!("`git branch {flag} {name}` exited with status {status}");
    }
    Ok(())
}

/// Short SHA a branch points at, for the ` (was …)` suffix that keeps a forced delete
/// recoverable from the reflog. `None` when the branch cannot be resolved.
/// The full `refs/heads/` form is queried: it is unambiguous against a same-named tag, and
/// it keeps a branch named `-x` from being read as an option.
pub(crate) fn short_sha(work: &Path, branch: &str) -> Option<String> {
    let refname = format!("refs/heads/{branch}");
    let sha = git_stdout(
        work,
        &["rev-parse", "--short", "--verify", "--quiet", &refname],
    )?;
    if sha.is_empty() { None } else { Some(sha) }
}

/// Commits reachable from the branch but from no other ref — what a forced delete would
/// actually lose (reflog aside). `0` means fully merged somewhere; `None` means the count
/// could not be computed (best-effort: no suffix is shown then).
/// `--exclude` takes a glob, but branch names cannot contain `*`, `?` or `[`
/// (`git check-ref-format`), so the refname is always a literal match.
pub(crate) fn unmerged_count(work: &Path, branch: &str) -> Option<usize> {
    let refname = format!("refs/heads/{branch}");
    let exclude = format!("--exclude={refname}");
    let count = git_stdout(
        work,
        &["rev-list", "--count", &refname, "--not", &exclude, "--all"],
    )?;

    count.parse().ok()
}

/// All values of a multi-valued `git config` key (repo-local, global and system levels
/// merged by git itself), each additionally split on commas. Empty when unset — `git
/// config` works from a non-repository directory too, reading the global/system levels.
pub(crate) fn config_values(work: &Path, key: &str) -> Vec<String> {
    git_stdout(work, &["config", "--get-all", key])
        .map(|text| {
            text.lines()
                .flat_map(|line| line.split(','))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Branch names checked out in any worktree of `work`. `git branch -D` refuses to delete
/// them, so they must not become candidates — the current worktree is only one of them.
fn checked_out_branches(work: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(text) = git_stdout(work, &["worktree", "list", "--porcelain"]) {
        // Porcelain format, not translated: `branch refs/heads/<name>` per attached worktree.
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("branch refs/heads/") {
                out.push(name.to_string());
            }
        }
    }
    // Detached HEAD makes this fail; the worktree list above already covers the rest.
    if let Some(current) = git_stdout(work, &["symbolic-ref", "--short", "HEAD"]) {
        if !current.is_empty() {
            out.push(current);
        }
    }
    out
}

fn gone_branches(work: &Path) -> Result<Detection> {
    // One line per branch: "<short-name>\t<upstream>".
    let out = Command::new(GIT)
        .args([
            "for-each-ref",
            "--format=%(refname:short)%09%(upstream)",
            "refs/heads/",
        ])
        .current_dir(work)
        .output()
        .context("failed to run `git for-each-ref`")?;
    if !out.status.success() {
        bail!(
            "`git for-each-ref` failed in {}: {}",
            work.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let mut detection = Detection::default();
    let mut tracked: Vec<(String, String)> = Vec::new();
    for raw in out.stdout.split(|b| *b == b'\n') {
        if raw.is_empty() {
            continue;
        }
        match parse_ref_line(raw) {
            RefLine::Tracked { branch, upstream } => {
                tracked.push((branch.to_string(), upstream.to_string()));
            }
            RefLine::Untracked => {}
            RefLine::Unparsable => detection.skipped.push(unparsable_name(raw)),
        }
    }
    if tracked.is_empty() {
        return Ok(detection);
    }

    let upstreams: Vec<&str> = tracked.iter().map(|(_, u)| u.as_str()).collect();
    let exists = resolve_refs(work, &upstreams)?;
    for ((branch, upstream), alive) in tracked.iter().zip(exists) {
        if !alive {
            detection.gone.push(branch.clone());
            detection.upstreams.insert(branch.clone(), upstream.clone());
        }
    }

    Ok(detection)
}

/// One line of `for-each-ref` output. A name with non-UTF-8 bytes or an embedded TAB
/// cannot be round-tripped back to Git: lossy decoding would query a ref that does not
/// exist and report a live branch as gone, so such lines are `Unparsable`.
#[derive(Debug, PartialEq, Eq)]
enum RefLine<'a> {
    Tracked { branch: &'a str, upstream: &'a str },
    Untracked,
    Unparsable,
}

fn parse_ref_line(raw: &[u8]) -> RefLine<'_> {
    let Ok(line) = std::str::from_utf8(raw) else {
        return RefLine::Unparsable;
    };
    let mut fields = line.split('\t');
    let (Some(branch), Some(upstream), None) = (fields.next(), fields.next(), fields.next()) else {
        return RefLine::Unparsable;
    };
    if upstream.is_empty() {
        return RefLine::Untracked;
    }

    RefLine::Tracked { branch, upstream }
}

/// Best-effort display form of a branch name that could not be parsed.
fn unparsable_name(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    match text.split_once('\t') {
        Some((name, _)) => name.to_string(),
        None => text.to_string(),
    }
}

/// Whether each ref resolves to a commit (`<ref>^{commit}`), in one
/// `git cat-file --batch-check` process instead of one `git rev-parse` per branch.
/// Locale-independent: ` missing` is plumbing output and never translated, and refnames
/// cannot contain a space or newline (`git check-ref-format`), so the suffix is unambiguous.
/// Anything other than `missing` (found, or e.g. `ambiguous`) counts as alive — the
/// conservative direction: an uncertain branch is kept, never deleted.
fn resolve_refs(work: &Path, refs: &[&str]) -> Result<Vec<bool>> {
    const CMD: &str = "`git cat-file --batch-check`";

    let mut input = String::new();
    for r in refs {
        input.push_str(r);
        input.push_str("^{commit}\n");
    }

    let mut child = Command::new(GIT)
        .args(["cat-file", "--batch-check"])
        .current_dir(work)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to run {CMD}"))?;
    let mut stdin = child.stdin.take().expect("stdin was piped");
    // Written from a thread: writing all queries first and reading only afterwards
    // deadlocks once either pipe buffer fills up on a repository with many branches.
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let out = child
        .wait_with_output()
        .with_context(|| format!("failed to run {CMD}"))?;
    // A write failure needs no separate handling: it leaves fewer output lines than
    // queries, which the length check below turns into an error.
    let _ = writer.join();
    if !out.status.success() {
        bail!(
            "{CMD} exited with status {} in {}",
            out.status,
            work.display()
        );
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let alive: Vec<bool> = text.lines().map(|l| !l.ends_with(" missing")).collect();
    if alive.len() != refs.len() {
        bail!(
            "{CMD} answered {} of {} queries in {}",
            alive.len(),
            refs.len(),
            work.display()
        );
    }

    Ok(alive)
}

/// Trimmed stdout of a successful `git` invocation, or `None` if it failed. Used for
/// queries where "could not answer" and "answered nothing" are equivalent.
fn git_stdout(work: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new(GIT)
        .args(args)
        .current_dir(work)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_tracked_branch() {
        assert_eq!(
            parse_ref_line(b"feature-x\trefs/remotes/origin/feature-x"),
            RefLine::Tracked {
                branch: "feature-x",
                upstream: "refs/remotes/origin/feature-x",
            }
        );
    }

    #[test]
    fn a_branch_without_upstream_is_untracked() {
        assert_eq!(parse_ref_line(b"main\t"), RefLine::Untracked);
    }

    #[test]
    fn non_utf8_and_tab_names_are_unparsable() {
        // Invalid UTF-8 in the branch name.
        assert_eq!(
            parse_ref_line(b"bad\xffbranch\trefs/remotes/origin/x"),
            RefLine::Unparsable
        );
        // An embedded TAB produces a third field and would otherwise be mis-split.
        assert_eq!(
            parse_ref_line(b"has\ttab\trefs/remotes/origin/x"),
            RefLine::Unparsable
        );
        // A line with no TAB at all is not the documented format either.
        assert_eq!(parse_ref_line(b"lonely"), RefLine::Unparsable);
    }

    #[test]
    fn a_dash_leading_name_parses_like_any_other() {
        // Such a ref cannot be created by `git branch`, but `git update-ref` can make one;
        // deletion passes it after `--`.
        assert_eq!(
            parse_ref_line(b"-x\trefs/remotes/origin/-x"),
            RefLine::Tracked {
                branch: "-x",
                upstream: "refs/remotes/origin/-x",
            }
        );
    }

    #[test]
    fn unparsable_name_takes_the_field_before_the_first_tab() {
        assert_eq!(unparsable_name(b"feat\tup\tstream"), "feat");
        assert_eq!(unparsable_name(b"feat"), "feat");
        assert_eq!(unparsable_name(b"ba\xffd\tup"), "ba\u{fffd}d");
    }
}
