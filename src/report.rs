use std::path::PathBuf;

use crate::git::Detection;

/// Per-repository outcome. The `Err` variant makes "could not be inspected" a property of
/// the type: an empty `gone` list can never be mistaken for a clean repository.
/// `fetch_error` records a failed `git fetch --prune` (offline, no remote): detection
/// still ran, but on possibly stale tracking refs — the report says so instead of
/// presenting the repository as verified-clean.
#[derive(Debug)]
pub(crate) struct RepoReport {
    pub(crate) path: PathBuf,
    pub(crate) result: Result<Detection, String>,
    pub(crate) fetch_error: Option<String>,
}

impl RepoReport {
    pub(crate) fn gone(&self) -> &[String] {
        self.result.as_ref().map_or(&[], |d| d.gone.as_slice())
    }

    /// Branches eligible for a destructive operation. A failed fetch means the tracking refs
    /// are known to be stale or incomplete, so they are useful in a report but unsafe to delete.
    pub(crate) fn deletable_gone(&self) -> &[String] {
        if self.fetch_error.is_some() {
            &[]
        } else {
            self.gone()
        }
    }

    pub(crate) fn skipped(&self) -> &[String] {
        self.result.as_ref().map_or(&[], |d| d.skipped.as_slice())
    }

    pub(crate) fn detection(&self) -> Option<&Detection> {
        self.result.as_ref().ok()
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.result.as_ref().err().map(String::as_str)
    }
}

/// Number of gone branches across the whole report.
pub(crate) fn total_gone(report: &[RepoReport]) -> usize {
    report.iter().map(|r| r.gone().len()).sum()
}

/// Number of repositories that have at least one gone branch.
pub(crate) fn repos_with_gone(report: &[RepoReport]) -> usize {
    report.iter().filter(|r| !r.gone().is_empty()).count()
}

/// Number of branches that can be deleted after a successful fetch.
pub(crate) fn total_deletable_gone(report: &[RepoReport]) -> usize {
    report.iter().map(|r| r.deletable_gone().len()).sum()
}

/// Number of repositories whose remote state could not be refreshed.
pub(crate) fn failed_fetches(report: &[RepoReport]) -> usize {
    report.iter().filter(|r| r.fetch_error.is_some()).count()
}

/// Number of repositories that could not be inspected.
pub(crate) fn failed_inspections(report: &[RepoReport]) -> usize {
    report.iter().filter(|r| r.error().is_some()).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(path: &str, gone: &[&str]) -> RepoReport {
        RepoReport {
            path: PathBuf::from(path),
            result: Ok(Detection {
                gone: gone.iter().map(|s| (*s).to_string()).collect(),
                skipped: Vec::new(),
                ..Detection::default()
            }),
            fetch_error: None,
        }
    }

    fn broken(path: &str) -> RepoReport {
        RepoReport {
            path: PathBuf::from(path),
            result: Err("`git for-each-ref` failed".to_string()),
            fetch_error: None,
        }
    }

    #[test]
    fn aggregates_counts_across_repositories() {
        let report = vec![ok("a", &["x", "y"]), ok("b", &[]), ok("c", &["z"])];

        assert_eq!(total_gone(&report), 3);
        assert_eq!(repos_with_gone(&report), 2);
        assert_eq!(failed_inspections(&report), 0);
    }

    #[test]
    fn a_broken_repository_counts_as_failed_and_contributes_no_branches() {
        let report = vec![ok("a", &["x"]), broken("b")];

        assert_eq!(total_gone(&report), 1);
        assert_eq!(repos_with_gone(&report), 1);
        assert_eq!(failed_inspections(&report), 1);
        assert!(report[1].gone().is_empty());
        assert_eq!(report[1].error(), Some("`git for-each-ref` failed"));
    }

    #[test]
    fn a_failed_fetch_keeps_branches_in_reports_but_not_in_deletion_candidates() {
        let mut repo = ok("a", &["stale"]);
        repo.fetch_error = Some("fetch failed".to_string());

        assert_eq!(repo.gone(), &["stale".to_string()]);
        assert!(repo.deletable_gone().is_empty());
        assert_eq!(total_deletable_gone(&[repo]), 0);
    }
}
