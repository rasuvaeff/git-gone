use std::num::NonZeroUsize;
use std::path::PathBuf;

use clap::Parser;

/// Directory names skipped during `--recursive` scan unless overridden by `--exclude`.
pub(crate) const DEFAULT_EXCLUDE: &[&str] = &[
    "target",
    "vendor",
    "node_modules",
    ".cache",
    "build",
    "dist",
];

#[derive(Parser, Debug)]
#[command(
    name = "git-gone",
    version,
    about = "Delete local Git branches whose remote has been deleted"
)]
pub(crate) struct Cli {
    /// List gone branches without deleting anything.
    #[arg(short, long)]
    pub(crate) list: bool,

    /// Show what would be deleted, but do not delete.
    #[arg(short = 'n', long = "dry-run")]
    pub(crate) dry_run: bool,

    /// Skip the confirmation prompt.
    #[arg(short = 'y', long)]
    pub(crate) yes: bool,

    /// Skip `git fetch --prune` before detecting.
    #[arg(long)]
    pub(crate) no_fetch: bool,

    /// Recursively scan subdirectories for nested Git repositories and operate on all of them.
    #[arg(short = 'r', long)]
    pub(crate) recursive: bool,

    /// Maximum scan depth with `--recursive` (0 = the scan root only, 1 = direct
    /// subdirectories). The scan root itself is always inspected. Default: unlimited.
    #[arg(long, requires = "recursive")]
    pub(crate) depth: Option<usize>,

    /// Directory tree to scan with `--recursive`. Default: the current directory.
    #[arg(long, value_name = "PATH", requires = "recursive")]
    pub(crate) root: Option<PathBuf>,

    /// Comma-separated directory names to skip during scan (replaces the default list
    /// and `git config gone.exclude`).
    #[arg(long, value_delimiter = ',', requires = "recursive")]
    pub(crate) exclude: Option<Vec<String>>,

    /// Repositories fetched in parallel with `--recursive`. Default: available
    /// CPUs, capped at 8.
    #[arg(short = 'j', long, requires = "recursive")]
    pub(crate) jobs: Option<NonZeroUsize>,

    /// Delete only branches Git considers fully merged (`git branch -d`).
    #[arg(long)]
    pub(crate) safe: bool,

    /// Permit nested Git metadata that resolves outside `--root` (for linked worktrees).
    #[arg(long, requires = "recursive")]
    pub(crate) allow_external_git_dir: bool,

    /// Comma-separated branch name patterns (`*` wildcard) that are never deletion
    /// candidates. Adds to `git config gone.protect`.
    #[arg(long, value_delimiter = ',')]
    pub(crate) protect: Option<Vec<String>>,

    /// Emit machine-readable JSON. Implies report mode (no deletion), so combining it
    /// with --yes would silently ignore the latter — rejected instead.
    #[arg(long, conflicts_with = "yes")]
    pub(crate) json: bool,

    /// Add upstream and unmerged-commit details to a JSON report.
    #[arg(long, requires = "json")]
    pub(crate) include_reasons: bool,
}

impl Cli {
    /// `--list` and `--dry-run` report without deleting. `--json` is handled earlier: it
    /// returns as soon as the document is printed.
    pub(crate) const fn is_report_only(&self) -> bool {
        self.list || self.dry_run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclude_and_protect_split_on_commas() {
        let cli = Cli::parse_from([
            "git-gone",
            "-r",
            "--exclude",
            "a,b",
            "--protect",
            "main,rel/*",
        ]);

        assert_eq!(cli.exclude, Some(vec!["a".to_string(), "b".to_string()]));
        assert_eq!(
            cli.protect,
            Some(vec!["main".to_string(), "rel/*".to_string()])
        );
    }

    #[test]
    fn jobs_requires_recursive_and_rejects_zero() {
        assert!(Cli::try_parse_from(["git-gone", "-j", "4"]).is_err());
        assert!(Cli::try_parse_from(["git-gone", "-r", "-j", "0"]).is_err());

        let cli = Cli::parse_from(["git-gone", "-r", "-j", "4"]);
        assert_eq!(cli.jobs.map(NonZeroUsize::get), Some(4));
    }

    #[test]
    fn list_and_dry_run_are_report_only() {
        for flag in ["--list", "--dry-run"] {
            let cli = Cli::parse_from(["git-gone", flag]);

            assert!(cli.is_report_only(), "{flag} must not delete");
        }
        assert!(!Cli::parse_from(["git-gone", "--yes"]).is_report_only());
    }

    #[test]
    fn json_conflicts_with_yes() {
        assert!(Cli::try_parse_from(["git-gone", "--json", "--yes"]).is_err());
        assert!(Cli::try_parse_from(["git-gone", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["git-gone", "--json", "--list"]).is_ok());
    }

    #[test]
    fn root_and_external_git_dir_require_recursive() {
        assert!(Cli::try_parse_from(["git-gone", "--root", "repos"]).is_err());
        assert!(Cli::try_parse_from(["git-gone", "--allow-external-git-dir"]).is_err());

        let cli = Cli::parse_from([
            "git-gone",
            "-r",
            "--root",
            "repos",
            "--allow-external-git-dir",
        ]);
        assert_eq!(cli.root, Some(PathBuf::from("repos")));
        assert!(cli.allow_external_git_dir);
    }

    #[test]
    fn include_reasons_requires_json() {
        assert!(Cli::try_parse_from(["git-gone", "--include-reasons"]).is_err());
        assert!(Cli::try_parse_from(["git-gone", "--json", "--include-reasons"]).is_ok());
    }

    #[test]
    fn depth_and_exclude_require_recursive() {
        assert!(Cli::try_parse_from(["git-gone", "--depth", "1"]).is_err());
        assert!(Cli::try_parse_from(["git-gone", "--exclude", "a"]).is_err());
        assert!(Cli::try_parse_from(["git-gone", "--root", "repos"]).is_err());
        assert!(Cli::try_parse_from(["git-gone", "-r", "--depth", "1"]).is_ok());
    }
}
