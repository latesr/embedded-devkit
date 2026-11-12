//! Copies the rule files and the shared xtask folder into other repos, and installs the git hooks there.
//!
//! The copy comes from the devkit's `HEAD`, so the commit in the other repo can name the devkit commit it holds. A
//! file in the other repo that matches no committed version of the devkit's file was edited in place. Sync reports
//! it and leaves it alone, so the edit can move to the devkit first.
//!
//! The hooks come from the shared `hooks` module that this binary is built from. Sync refuses to run while that
//! module has changes that are not committed, so the hooks match the copied files.

use crate::shared::{files::RULE_FILES, git, hooks};
use std::fs;
use std::path::{Path, PathBuf};

/// The folder that every repo holds a copy of, relative to each repo's root.
const SHARED: &str = "xtask/src/shared";

/// Every path that sync copies. A file here that the devkit deletes stays in the other repos, because only
/// [`SHARED`] is scanned for files to remove.
fn synced() -> Vec<&'static str> {
    RULE_FILES.iter().copied().chain([SHARED]).collect()
}

/// What one sync changed in one repo, and what it left alone. Each entry is a path relative to the repo's root.
#[derive(Debug, Default, PartialEq)]
struct Report {
    added: Vec<String>,
    updated: Vec<String>,
    removed: Vec<String>,
    edited_in_place: Vec<String>,
    /// False for a folder that is not a git repository yet.
    hooks_installed: bool,
}

impl Report {
    /// The lines that tell the user what changed. The copies edited in place are reported apart, as the failure.
    fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (what, paths) in [
            ("added", &self.added),
            ("updated", &self.updated),
            ("removed", &self.removed),
        ] {
            lines.extend(paths.iter().map(|path| format!("{what} {path}")));
        }
        lines.push(if self.hooks_installed {
            "installed the git hooks".to_owned()
        } else {
            "no .git/hooks folder, so the git hooks were not installed".to_owned()
        });
        lines
    }
}

/// Syncs each repo in `repos` from the devkit at `devkit`. Fails when the devkit's shared files have changes that
/// are not committed, or when a repo holds a copy that was edited in place.
pub fn run(devkit: &Path, repos: &[PathBuf]) -> Result<(), String> {
    let status = [&["status", "--porcelain", "--"][..], &synced()].concat();
    if !git(devkit, &status)?.is_empty() {
        return Err(
            "The devkit's rule files or xtask/src/shared have changes that are not committed. Commit them, then sync."
                .to_owned(),
        );
    }
    let head = String::from_utf8_lossy(&git(devkit, &["rev-parse", "--short", "HEAD"])?)
        .trim()
        .to_owned();
    let mut edited = Vec::new();
    for repo in repos {
        println!("==> {}", repo.display());
        let report = sync_one(devkit, repo)?;
        for line in report.lines() {
            println!("{line}");
        }
        for path in report.edited_in_place {
            edited.push(format!("{}: {path}", repo.display()));
        }
    }
    println!("synced from embedded-devkit {head}");
    if edited.is_empty() {
        return Ok(());
    }
    Err(format!(
        "These copies match no committed version in the devkit, so sync left them alone. Move each edit to the \
         devkit, commit it, and sync again.\n{}",
        edited.join("\n")
    ))
}

fn sync_one(devkit: &Path, repo: &Path) -> Result<Report, String> {
    let mut report = Report::default();
    let ls_tree = [
        &["ls-tree", "-r", "--name-only", "HEAD", "--"][..],
        &synced(),
    ]
    .concat();
    let listing = git(devkit, &ls_tree)?;
    let current: Vec<String> = String::from_utf8_lossy(&listing)
        .lines()
        .map(str::to_owned)
        .collect();
    for path in &current {
        let content = git(devkit, &["show", &format!("HEAD:{path}")])?;
        let target = repo.join(path);
        match fs::read(&target) {
            Ok(existing) if existing == content => continue,
            Ok(existing) if !history(devkit, path)?.contains(&existing) => {
                report.edited_in_place.push(path.clone());
                continue;
            }
            Ok(_) => report.updated.push(path.clone()),
            Err(_) => report.added.push(path.clone()),
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        fs::write(&target, content).map_err(|error| format!("{}: {error}", target.display()))?;
    }
    let entries = fs::read_dir(repo.join(SHARED))
        .into_iter()
        .flatten()
        .flatten();
    for entry in entries {
        let path = format!("{SHARED}/{}", entry.file_name().to_string_lossy());
        if current.contains(&path) || !entry.path().is_file() {
            continue;
        }
        let existing = fs::read(entry.path()).map_err(|error| format!("{path}: {error}"))?;
        if history(devkit, &path)?.contains(&existing) {
            fs::remove_file(entry.path()).map_err(|error| format!("{path}: {error}"))?;
            report.removed.push(path);
        } else {
            report.edited_in_place.push(path);
        }
    }
    report.removed.sort();
    report.edited_in_place.sort();
    if repo.join(".git").join("hooks").is_dir() {
        hooks::install(repo)?;
        report.hooks_installed = true;
    }
    Ok(report)
}

/// Every committed version of `path` in the devkit. The commit that deleted the file has no version, so it is
/// skipped.
fn history(devkit: &Path, path: &str) -> Result<Vec<Vec<u8>>, String> {
    let commits = git(devkit, &["log", "--format=%H", "--", path])?;
    Ok(String::from_utf8_lossy(&commits)
        .lines()
        .filter_map(|commit| git(devkit, &["show", &format!("{commit}:{path}")]).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::files::testing::TempRepo;

    /// A devkit with two commits. `a.rs` changed between them, `old.rs` was removed and `new.rs` was added.
    fn devkit(name: &str) -> TempRepo {
        let devkit = TempRepo::new(&format!("{name}-devkit"));
        fs::create_dir_all(devkit.0.join(SHARED)).unwrap();
        for args in [
            &["init", "--quiet"][..],
            &["config", "user.email", "test@example.com"],
            &["config", "user.name", "test"],
        ] {
            git(&devkit.0, args).unwrap();
        }
        devkit.write("xtask/src/shared/a.rs", "a1\n");
        devkit.write("xtask/src/shared/old.rs", "old\n");
        git(&devkit.0, &["add", "--all"]).unwrap();
        git(&devkit.0, &["commit", "--quiet", "-m", "one"]).unwrap();
        devkit.write("xtask/src/shared/a.rs", "a2\n");
        devkit.write("xtask/src/shared/new.rs", "new\n");
        fs::remove_file(devkit.0.join("xtask/src/shared/old.rs")).unwrap();
        git(&devkit.0, &["add", "--all"]).unwrap();
        git(&devkit.0, &["commit", "--quiet", "-m", "two"]).unwrap();
        devkit
    }

    fn repo(name: &str) -> TempRepo {
        let repo = TempRepo::new(name);
        fs::create_dir_all(repo.0.join(SHARED)).unwrap();
        repo
    }

    fn read(repo: &TempRepo, path: &str) -> Option<String> {
        fs::read_to_string(repo.0.join(path)).ok()
    }

    fn paths(names: &[&str]) -> Vec<String> {
        names
            .iter()
            .map(|name| format!("{SHARED}/{name}"))
            .collect()
    }

    #[test]
    fn an_old_copy_is_brought_up_to_date() {
        let devkit = devkit("sync-old-copy");
        let repo = repo("sync-old-copy");
        repo.write("xtask/src/shared/a.rs", "a1\n");
        repo.write("xtask/src/shared/old.rs", "old\n");

        let report = sync_one(&devkit.0, &repo.0).unwrap();
        assert_eq!(
            report,
            Report {
                added: paths(&["new.rs"]),
                updated: paths(&["a.rs"]),
                removed: paths(&["old.rs"]),
                edited_in_place: Vec::new(),
                hooks_installed: false,
            }
        );
        assert_eq!(
            read(&repo, "xtask/src/shared/a.rs").as_deref(),
            Some("a2\n")
        );
        assert_eq!(
            read(&repo, "xtask/src/shared/new.rs").as_deref(),
            Some("new\n")
        );
        assert_eq!(read(&repo, "xtask/src/shared/old.rs"), None);
    }

    #[test]
    fn a_current_copy_is_left_alone_and_reported_as_nothing() {
        let devkit = devkit("sync-current");
        let repo = repo("sync-current");
        repo.write("xtask/src/shared/a.rs", "a2\n");
        repo.write("xtask/src/shared/new.rs", "new\n");
        assert_eq!(sync_one(&devkit.0, &repo.0).unwrap(), Report::default());
    }

    #[test]
    fn a_copy_edited_in_place_is_reported_and_kept() {
        let devkit = devkit("sync-edited");
        let repo = repo("sync-edited");
        repo.write("xtask/src/shared/a.rs", "a1 edited\n");
        repo.write("xtask/src/shared/mine.rs", "mine\n");

        let report = sync_one(&devkit.0, &repo.0).unwrap();
        assert_eq!(report.edited_in_place, paths(&["a.rs", "mine.rs"]));
        assert_eq!(report.added, paths(&["new.rs"]));
        assert_eq!(
            read(&repo, "xtask/src/shared/a.rs").as_deref(),
            Some("a1 edited\n")
        );
        assert_eq!(
            read(&repo, "xtask/src/shared/mine.rs").as_deref(),
            Some("mine\n")
        );
    }

    #[test]
    fn a_repo_without_the_folder_gets_every_file() {
        let devkit = devkit("sync-empty");
        let repo = TempRepo::new("sync-empty");
        let report = sync_one(&devkit.0, &repo.0).unwrap();
        assert_eq!(report.added, paths(&["a.rs", "new.rs"]));
        assert_eq!(
            read(&repo, "xtask/src/shared/a.rs").as_deref(),
            Some("a2\n")
        );
        assert!(!report.hooks_installed);
    }

    #[test]
    fn the_report_names_each_change_and_a_missing_hooks_folder() {
        let report = Report {
            added: vec!["a".to_owned()],
            updated: vec!["u".to_owned()],
            removed: vec!["r".to_owned()],
            edited_in_place: vec!["e".to_owned()],
            hooks_installed: false,
        };
        assert_eq!(
            report.lines(),
            [
                "added a",
                "updated u",
                "removed r",
                "no .git/hooks folder, so the git hooks were not installed",
            ]
        );
        let installed = Report {
            hooks_installed: true,
            ..Report::default()
        };
        assert_eq!(installed.lines(), ["installed the git hooks"]);
    }

    #[test]
    fn the_git_hooks_are_installed_in_a_git_repository() {
        let devkit = devkit("sync-hooks");
        let repo = repo("sync-hooks");
        fs::create_dir_all(repo.0.join(".git/hooks")).unwrap();
        repo.write(".git/hooks/commit-msg", "old\n");

        let report = sync_one(&devkit.0, &repo.0).unwrap();
        assert!(report.hooks_installed);
        let commit_msg = read(&repo, ".git/hooks/commit-msg").unwrap();
        assert!(
            commit_msg.contains("cargo xtask links --message"),
            "{commit_msg}"
        );
        let pre_commit = read(&repo, ".git/hooks/pre-commit").unwrap();
        assert!(pre_commit.contains("cargo xtask check"), "{pre_commit}");
    }

    #[test]
    fn run_fails_on_uncommitted_devkit_changes_and_on_an_edited_copy() {
        let devkit = devkit("sync-run");
        let repo = repo("sync-run");
        let repos = [repo.0.clone()];
        repo.write("xtask/src/shared/a.rs", "a1 edited\n");
        let error = run(&devkit.0, &repos).unwrap_err();
        assert!(error.contains("xtask/src/shared/a.rs"), "{error}");
        assert_eq!(
            read(&repo, "xtask/src/shared/new.rs").as_deref(),
            Some("new\n")
        );

        repo.write("xtask/src/shared/a.rs", "a2\n");
        assert_eq!(run(&devkit.0, &repos), Ok(()));

        devkit.write("xtask/src/shared/a.rs", "a3\n");
        let error = run(&devkit.0, &repos).unwrap_err();
        assert!(error.contains("not committed"), "{error}");
        assert_eq!(
            read(&repo, "xtask/src/shared/a.rs").as_deref(),
            Some("a2\n")
        );
    }

    #[test]
    fn the_rule_files_are_synced_to_the_repo_root() {
        let devkit = devkit("sync-conventions");
        let rule_files = [
            "CHECKS.md",
            "CODE.md",
            "CONTRIBUTING.md",
            "CONVENTIONS.md",
            "DOCS.md",
            "EMBEDDED.md",
            "TESTING.md",
            "WRITING.md",
        ];
        for file in rule_files {
            devkit.write(file, &format!("{file}\n"));
        }
        devkit.write("README.md", "the devkit's own\n");
        git(&devkit.0, &["add", "--all"]).unwrap();
        git(&devkit.0, &["commit", "--quiet", "-m", "three"]).unwrap();
        let repo = repo("sync-conventions");
        let repos = [repo.0.clone()];

        let report = sync_one(&devkit.0, &repo.0).unwrap();
        let mut expected: Vec<String> = rule_files.iter().map(|file| (*file).to_owned()).collect();
        expected.extend(paths(&["a.rs", "new.rs"]));
        assert_eq!(report.added, expected);
        assert_eq!(read(&repo, "README.md"), None);
        assert_eq!(
            read(&repo, "CONVENTIONS.md").as_deref(),
            Some("CONVENTIONS.md\n")
        );
        assert_eq!(read(&repo, "WRITING.md").as_deref(), Some("WRITING.md\n"));

        repo.write("CONVENTIONS.md", "rules edited\n");
        let report = sync_one(&devkit.0, &repo.0).unwrap();
        assert_eq!(report.edited_in_place, vec!["CONVENTIONS.md".to_owned()]);

        repo.write("CONVENTIONS.md", "CONVENTIONS.md\n");
        devkit.write("CONVENTIONS.md", "rules changed\n");
        let error = run(&devkit.0, &repos).unwrap_err();
        assert!(error.contains("not committed"), "{error}");
    }
}
