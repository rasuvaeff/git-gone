use std::fs;
use std::path::{Path, PathBuf};

/// Repositories found by the directory scan, plus the directories that could not be read.
/// A scan error means coverage is incomplete, so it is reported and affects the exit code —
/// an unreadable directory must never look like "no repositories here".
#[derive(Debug, Default)]
pub(crate) struct Scan {
    pub(crate) repos: Vec<PathBuf>,
    pub(crate) errors: Vec<String>,
}

/// Recursively collects Git repository paths starting at `root`.
/// A directory containing `.git` is added; unlike a naive leaf scan, descent continues
/// so that nested repositories (a meta-repo with per-package repos, submodules, etc.)
/// are all discovered.
pub(crate) fn find_repos(
    root: &Path,
    depth: Option<usize>,
    exclude: &[String],
    allow_external_git_dir: bool,
) -> Scan {
    let mut scan = Scan::default();
    let canonical_root = match root.canonicalize() {
        Ok(path) => path,
        Err(e) => {
            scan.errors.push(format!(
                "could not resolve scan root {}: {e}",
                root.display()
            ));

            return scan;
        }
    };
    walk(
        root,
        &canonical_root,
        depth,
        exclude,
        allow_external_git_dir,
        &mut scan,
    );
    scan.repos.sort();
    scan.repos.dedup();
    scan
}

/// Display a repo path relative to the scan root. The root itself renders as `.`.
pub(crate) fn rel_path(repo: &Path, root: &Path) -> String {
    if repo == root {
        ".".to_string()
    } else {
        display_path(repo.strip_prefix(root).unwrap_or(repo))
    }
}

fn display_path(path: &Path) -> String {
    #[cfg(windows)]
    {
        path.display().to_string().replace('\\', "/")
    }

    #[cfg(not(windows))]
    {
        path.display().to_string()
    }
}

fn walk(
    dir: &Path,
    canonical_root: &Path,
    depth: Option<usize>,
    exclude: &[String],
    allow_external_git_dir: bool,
    scan: &mut Scan,
) {
    match git_dir_is_within_root(dir, canonical_root, allow_external_git_dir) {
        Ok(true) => {
            scan.repos.push(dir.to_path_buf());
            // Keep descending: the parent may itself be a meta-repo wrapping per-package repos.
        }
        Ok(false) => {}
        Err(e) => scan.errors.push(e),
    }
    match depth {
        None => descend(
            dir,
            canonical_root,
            None,
            exclude,
            allow_external_git_dir,
            scan,
        ),
        Some(0) => {}
        Some(remaining) => descend(
            dir,
            canonical_root,
            Some(remaining - 1),
            exclude,
            allow_external_git_dir,
            scan,
        ),
    }
}

fn descend(
    dir: &Path,
    canonical_root: &Path,
    next_depth: Option<usize>,
    exclude: &[String],
    allow_external_git_dir: bool,
    scan: &mut Scan,
) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            scan.errors
                .push(format!("could not read {}: {e}", dir.display()));

            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                scan.errors
                    .push(format!("could not read an entry in {}: {e}", dir.display()));
                continue;
            }
        };
        // `read_dir` does not follow symlinks, so a symlinked directory reports
        // `is_dir() == false` and is skipped — which also makes traversal cycles impossible.
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                scan.errors
                    .push(format!("could not stat {}: {e}", entry.path().display()));
                continue;
            }
        };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_lossy = name.to_string_lossy();
        if name_lossy.starts_with('.') {
            continue;
        }
        if exclude.iter().any(|e| e == &*name_lossy) {
            continue;
        }
        walk(
            &entry.path(),
            canonical_root,
            next_depth,
            exclude,
            allow_external_git_dir,
            scan,
        );
    }
}

/// Returns whether `dir` is a repository whose Git metadata, including a worktree's common
/// directory, is physically contained by the requested scan root. A nested gitfile may point
/// outside the tree; treating that directory as a repository would make `git branch -D` mutate
/// refs the operator did not ask to scan.
fn git_dir_is_within_root(
    dir: &Path,
    canonical_root: &Path,
    allow_external_git_dir: bool,
) -> Result<bool, String> {
    let metadata_path = dir.join(".git");
    let metadata = match fs::symlink_metadata(&metadata_path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(format!(
                "could not inspect Git metadata in {}: {e}",
                dir.display()
            ));
        }
    };
    let git_dir = if metadata.file_type().is_file() {
        git_dir_from_file(dir, &metadata_path)?
    } else if metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        metadata_path
    } else {
        return Err(format!("unsupported Git metadata in {}", dir.display()));
    };
    let canonical_git_dir = git_dir
        .canonicalize()
        .map_err(|e| format!("could not resolve Git metadata in {}: {e}", dir.display()))?;
    if !canonical_git_dir.is_dir() {
        return Err(format!(
            "Git metadata in {} is not a directory",
            dir.display()
        ));
    }
    ensure_within_root(
        &canonical_git_dir,
        canonical_root,
        dir,
        allow_external_git_dir,
    )?;

    let common_dir_file = canonical_git_dir.join("commondir");
    match fs::symlink_metadata(&common_dir_file) {
        Ok(_) => {
            let common_dir = common_dir_from_file(&canonical_git_dir, &common_dir_file)?;
            let canonical_common_dir = common_dir.canonicalize().map_err(|e| {
                format!(
                    "could not resolve common Git metadata in {}: {e}",
                    dir.display()
                )
            })?;
            ensure_within_root(
                &canonical_common_dir,
                canonical_root,
                dir,
                allow_external_git_dir,
            )?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "could not inspect common Git metadata in {}: {e}",
                dir.display()
            ));
        }
    }

    Ok(true)
}

fn git_dir_from_file(base: &Path, file: &Path) -> Result<PathBuf, String> {
    let content = fs::read_to_string(file)
        .map_err(|e| format!("could not read Git metadata {}: {e}", file.display()))?;
    let Some(value) = content.strip_prefix("gitdir: ") else {
        return Err(format!("invalid Git metadata file {}", file.display()));
    };
    let value = value.lines().next().unwrap_or_default();
    if value.is_empty() {
        return Err(format!("invalid Git metadata file {}", file.display()));
    }
    let path = PathBuf::from(value);

    Ok(if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}

fn common_dir_from_file(base: &Path, file: &Path) -> Result<PathBuf, String> {
    let content = fs::read_to_string(file)
        .map_err(|e| format!("could not read common Git metadata {}: {e}", file.display()))?;
    let value = content.lines().next().unwrap_or_default();
    if value.is_empty() {
        return Err(format!(
            "invalid common Git metadata file {}",
            file.display()
        ));
    }
    let path = PathBuf::from(value);

    Ok(if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}

fn ensure_within_root(
    path: &Path,
    canonical_root: &Path,
    repo: &Path,
    allow_external_git_dir: bool,
) -> Result<(), String> {
    if allow_external_git_dir || path.starts_with(canonical_root) {
        Ok(())
    } else {
        Err(format!(
            "skipping {}: Git metadata resolves outside scan root",
            repo.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    /// `<root>/a`, `<root>/skip`, `<root>/nested/deep` are repositories;
    /// `<root>/.hidden` is one too, but dot-directories are never entered.
    fn fixture() -> TempDir {
        let tmp = TempDir::new().unwrap();
        for rel in ["a/.git", "skip/.git", "nested/deep/.git", ".hidden/.git"] {
            fs::create_dir_all(tmp.path().join(rel)).unwrap();
        }
        tmp
    }

    fn found(scan: &Scan, root: &Path) -> Vec<String> {
        scan.repos.iter().map(|p| rel_path(p, root)).collect()
    }

    #[test]
    fn descends_into_nested_repositories_and_skips_dot_dirs() {
        let tmp = fixture();
        let scan = find_repos(tmp.path(), None, &[], false);

        assert_eq!(found(&scan, tmp.path()), vec!["a", "nested/deep", "skip"]);
        assert!(scan.errors.is_empty(), "errors: {:?}", scan.errors);
    }

    #[test]
    fn a_repository_wrapping_repositories_is_not_a_leaf() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        fs::create_dir_all(tmp.path().join("pkg/.git")).unwrap();
        let scan = find_repos(tmp.path(), None, &[], false);

        assert_eq!(found(&scan, tmp.path()), vec![".", "pkg"]);
    }

    #[test]
    fn depth_limits_descent() {
        let tmp = fixture();

        assert_eq!(
            found(&find_repos(tmp.path(), Some(1), &[], false), tmp.path()),
            vec!["a", "skip"],
            "depth 1 must not reach nested/deep"
        );
        assert!(
            find_repos(tmp.path(), Some(0), &[], false).repos.is_empty(),
            "depth 0 = the scan root only, which is not a repository here"
        );
    }

    #[test]
    fn the_scan_root_is_inspected_at_any_depth() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();

        assert_eq!(
            found(&find_repos(tmp.path(), Some(0), &[], false), tmp.path()),
            vec!["."]
        );
    }

    #[test]
    fn exclude_skips_named_directories() {
        let tmp = fixture();
        let scan = find_repos(tmp.path(), None, &["skip".to_string()], false);

        assert_eq!(found(&scan, tmp.path()), vec!["a", "nested/deep"]);
    }

    #[test]
    fn an_unreadable_root_is_an_error_not_an_empty_result() {
        let tmp = TempDir::new().unwrap();
        let scan = find_repos(&tmp.path().join("nope"), None, &[], false);

        assert!(scan.repos.is_empty());
        assert_eq!(scan.errors.len(), 1, "errors: {:?}", scan.errors);
        assert!(scan.errors[0].contains("could not resolve scan root"));
    }

    #[test]
    fn accepts_a_gitfile_whose_metadata_stays_within_the_scan_root() {
        let tmp = TempDir::new().unwrap();
        let git_dir = tmp.path().join("metadata");
        let repo = tmp.path().join("pkg");
        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join(".git"), "gitdir: ../metadata\n").unwrap();

        let scan = find_repos(tmp.path(), None, &[], false);

        assert_eq!(found(&scan, tmp.path()), vec!["pkg"]);
        assert!(scan.errors.is_empty(), "errors: {:?}", scan.errors);
    }

    #[test]
    fn rejects_a_gitfile_that_points_outside_the_scan_root() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let repo = tmp.path().join("attacker");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(outside.path().join(".git")).unwrap();
        fs::write(
            repo.join(".git"),
            format!("gitdir: {}\n", outside.path().join(".git").display()),
        )
        .unwrap();

        let scan = find_repos(tmp.path(), None, &[], false);

        assert!(scan.repos.is_empty());
        assert!(
            scan.errors
                .iter()
                .any(|error| error.contains("outside scan root")),
            "errors: {:?}",
            scan.errors
        );
    }

    #[test]
    fn allows_external_git_metadata_only_with_explicit_opt_in() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let repo = tmp.path().join("linked-worktree");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(outside.path().join(".git")).unwrap();
        fs::write(
            repo.join(".git"),
            format!("gitdir: {}\n", outside.path().join(".git").display()),
        )
        .unwrap();

        let scan = find_repos(tmp.path(), None, &[], true);

        assert_eq!(found(&scan, tmp.path()), vec!["linked-worktree"]);
        assert!(scan.errors.is_empty(), "errors: {:?}", scan.errors);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_git_metadata_symlink_that_points_outside_the_scan_root() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let repo = tmp.path().join("attacker");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(outside.path().join(".git")).unwrap();
        symlink(outside.path().join(".git"), repo.join(".git")).unwrap();

        let scan = find_repos(tmp.path(), None, &[], false);

        assert!(scan.repos.is_empty());
        assert!(
            scan.errors
                .iter()
                .any(|error| error.contains("outside scan root")),
            "errors: {:?}",
            scan.errors
        );
    }

    #[test]
    fn rejects_a_common_git_dir_that_points_outside_the_scan_root() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let repo = tmp.path().join("attacker");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(outside.path().join("common")).unwrap();
        fs::write(
            repo.join(".git/commondir"),
            format!("{}\n", outside.path().join("common").display()),
        )
        .unwrap();

        let scan = find_repos(tmp.path(), None, &[], false);

        assert!(scan.repos.is_empty());
        assert!(
            scan.errors
                .iter()
                .any(|error| error.contains("outside scan root")),
            "errors: {:?}",
            scan.errors
        );
    }

    #[test]
    fn rel_path_renders_the_root_as_dot() {
        let root = Path::new("/tmp/scan");

        assert_eq!(rel_path(root, root), ".");
        assert_eq!(rel_path(&root.join("pkg-a"), root), "pkg-a");
        // A path outside the root is rendered in full rather than mangled.
        assert_eq!(rel_path(Path::new("/elsewhere"), root), "/elsewhere");
    }

    /// A directory the process cannot read must surface as an error: silently skipping it
    /// would hide every repository underneath.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_subdirectory_is_reported() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let locked = tmp.path().join("locked");
        fs::create_dir_all(locked.join("pkg/.git")).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let scan = find_repos(tmp.path(), None, &[], false);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        // Running as root defeats the permission bits; then there is nothing to assert.
        if scan.errors.is_empty() {
            assert_eq!(found(&scan, tmp.path()), vec!["locked/pkg"]);
            return;
        }
        assert!(
            scan.errors.iter().any(|e| e.contains("locked")),
            "errors: {:?}",
            scan.errors
        );
        assert!(scan.repos.is_empty());
    }
}
