//! Hand-rolled JSON output — no serde, to keep the dependency set at two crates.
//! The documents are small and fully known here; only escaping needs care.

use std::path::Path;

use crate::git::{self, Detection};
use crate::report::RepoReport;
use crate::scan::rel_path;

/// Single-repository document. Always emitted, including for an empty result — consumers
/// pipe stdout into `jq` unconditionally. `skipped` carries branches whose name could not
/// be parsed: they reach the machine consumer too, not only stderr.
pub(crate) fn single_document(gone: &[String], skipped: &[String]) -> String {
    format!(
        "{{\"gone\":[{}],\"skipped\":[{}]}}",
        array(gone),
        array(skipped)
    )
}

/// Single-repository document with optional per-branch context. The existing fields retain
/// their shape; `reasons` is an additive field for consumers that opt in.
pub(crate) fn single_document_with_reasons(work: &Path, detection: &Detection) -> String {
    format!(
        "{{\"gone\":[{}],\"skipped\":[{}],\"reasons\":[{}]}}",
        array(&detection.gone),
        array(&detection.skipped),
        reasons(work, detection),
    )
}

/// Multi-repository document. `error` is `null` for a healthy repository and carries the
/// failure message otherwise, so an empty `gone` never masks a failed inspection.
/// `fetch_error` is `null` when `git fetch --prune` succeeded (or was skipped via
/// `--no-fetch`) and carries the failure message otherwise: detection then ran on possibly
/// stale tracking refs, and a consumer must be able to tell that from verified-clean.
pub(crate) fn multi_document(root: &Path, report: &[RepoReport], total: usize) -> String {
    multi_document_inner(root, report, total, false)
}

/// Multi-repository document with additive per-branch context for `--include-reasons`.
pub(crate) fn multi_document_with_reasons(
    root: &Path,
    report: &[RepoReport],
    total: usize,
) -> String {
    multi_document_inner(root, report, total, true)
}

fn multi_document_inner(
    root: &Path,
    report: &[RepoReport],
    total: usize,
    include_reasons: bool,
) -> String {
    if report.is_empty() {
        return "{\n  \"repositories\":[],\n  \"total_gone\":0\n}".to_string();
    }
    let entries: Vec<String> = report
        .iter()
        .map(|r| {
            let error = nullable(r.error());
            let fetch_error = nullable(r.fetch_error.as_deref());
            let reasons = if include_reasons {
                r.detection()
                    .map_or_else(|| "[]".to_string(), |detection| reasons(&r.path, detection))
            } else {
                String::new()
            };
            let reasons_field = if include_reasons {
                format!(",\"reasons\":[{reasons}]")
            } else {
                String::new()
            };
            format!(
                "    {{\"path\":\"{}\",\"gone\":[{}],\"skipped\":[{}],\"error\":{error},\"fetch_error\":{fetch_error}{reasons_field}}}",
                escape(&rel_path(&r.path, root)),
                array(r.gone()),
                array(r.skipped()),
            )
        })
        .collect();

    format!(
        "{{\n  \"repositories\":[\n{}\n  ],\n  \"total_gone\":{total}\n}}",
        entries.join(",\n"),
    )
}

fn reasons(work: &Path, detection: &Detection) -> String {
    detection
        .gone
        .iter()
        .map(|branch| {
            let upstream = detection.upstream(branch).unwrap_or_default();
            let unmerged = git::unmerged_count(work, branch)
                .map_or_else(|| "null".to_string(), |count| count.to_string());
            format!(
                "{{\"branch\":\"{}\",\"upstream\":\"{}\",\"unmerged_commits\":{unmerged}}}",
                escape(branch),
                escape(upstream),
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn nullable(value: Option<&str>) -> String {
    match value {
        Some(v) => format!("\"{}\"", escape(v)),
        None => "null".to_string(),
    }
}

fn array(items: &[String]) -> String {
    items
        .iter()
        .map(|i| format!("\"{}\"", escape(i)))
        .collect::<Vec<_>>()
        .join(",")
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use std::collections::BTreeMap;

    fn ok(path: &str, gone: &[&str]) -> RepoReport {
        RepoReport {
            path: PathBuf::from(path),
            result: Ok(Detection {
                gone: gone.iter().map(|s| (*s).to_string()).collect(),
                skipped: Vec::new(),
                upstreams: BTreeMap::new(),
            }),
            fetch_error: None,
        }
    }

    #[test]
    fn escapes_quotes_backslashes_and_control_characters() {
        assert_eq!(escape("plain"), "plain");
        assert_eq!(escape("a\"b"), "a\\\"b");
        assert_eq!(escape("a\\b"), "a\\\\b");
        assert_eq!(escape("a\nb\rc\td"), "a\\nb\\rc\\td");
        assert_eq!(escape("\u{1}"), "\\u0001");
        assert_eq!(escape("\u{1f}"), "\\u001f");
        // Printable and non-ASCII characters pass through as UTF-8.
        assert_eq!(escape(" ветка/x"), " ветка/x");
    }

    #[test]
    fn array_quotes_and_joins() {
        assert_eq!(array(&[]), "");
        assert_eq!(array(&["a".to_string()]), "\"a\"");
        assert_eq!(
            array(&["a".to_string(), "b\"c".to_string()]),
            "\"a\",\"b\\\"c\""
        );
    }

    #[test]
    fn single_document_is_printed_even_when_empty() {
        assert_eq!(single_document(&[], &[]), "{\"gone\":[],\"skipped\":[]}");
        assert_eq!(
            single_document(&["feature-x".to_string()], &["bad\u{fffd}".to_string()]),
            "{\"gone\":[\"feature-x\"],\"skipped\":[\"bad\u{fffd}\"]}"
        );
    }

    #[test]
    fn reasons_are_an_additive_json_field() {
        let detection = Detection {
            gone: vec!["feature-x".to_string()],
            skipped: Vec::new(),
            upstreams: BTreeMap::from([(
                "feature-x".to_string(),
                "refs/remotes/origin/feature-x".to_string(),
            )]),
        };
        let tmp = tempfile::TempDir::new().unwrap();

        assert_eq!(
            single_document_with_reasons(tmp.path(), &detection),
            concat!(
                "{\"gone\":[\"feature-x\"],\"skipped\":[],\"reasons\":[",
                "{\"branch\":\"feature-x\",\"upstream\":\"refs/remotes/origin/feature-x\",",
                "\"unmerged_commits\":null}]}"
            )
        );
    }

    #[test]
    fn multi_document_is_printed_even_when_no_repositories_were_found() {
        let doc = multi_document(Path::new("/root"), &[], 0);

        assert_eq!(doc, "{\n  \"repositories\":[],\n  \"total_gone\":0\n}");
    }

    #[test]
    fn multi_document_carries_paths_branches_and_errors() {
        let report = vec![
            ok("/root/pkg-a", &["feat/x"]),
            RepoReport {
                path: PathBuf::from("/root/pkg-broken"),
                result: Err("`git for-each-ref` failed in \"quoted\"".to_string()),
                fetch_error: None,
            },
        ];
        let doc = multi_document(Path::new("/root"), &report, 1);

        assert_eq!(
            doc,
            concat!(
                "{\n  \"repositories\":[\n",
                "    {\"path\":\"pkg-a\",\"gone\":[\"feat/x\"],\"skipped\":[],",
                "\"error\":null,\"fetch_error\":null},\n",
                "    {\"path\":\"pkg-broken\",\"gone\":[],\"skipped\":[],",
                "\"error\":\"`git for-each-ref` failed in \\\"quoted\\\"\",",
                "\"fetch_error\":null}\n",
                "  ],\n  \"total_gone\":1\n}"
            )
        );
    }

    /// A failed fetch means detection ran on possibly stale tracking refs; the document
    /// must say so — `"gone":[]` with a failed fetch is not the same as verified-clean.
    #[test]
    fn multi_document_carries_a_fetch_error() {
        let mut offline = ok("/root/pkg-offline", &[]);
        offline.fetch_error = Some("`git fetch --prune` exited with status 128".to_string());
        let doc = multi_document(Path::new("/root"), &[offline], 0);

        assert_eq!(
            doc,
            concat!(
                "{\n  \"repositories\":[\n",
                "    {\"path\":\"pkg-offline\",\"gone\":[],\"skipped\":[],\"error\":null,",
                "\"fetch_error\":\"`git fetch --prune` exited with status 128\"}\n",
                "  ],\n  \"total_gone\":0\n}"
            )
        );
    }
}
