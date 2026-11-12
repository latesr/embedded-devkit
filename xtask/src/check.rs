//! Runs the checks. The link and prose checks go first, because they take under a second and a writing fault
//! should fail before the builds.

use crate::shared;
use crate::shared::checks::{
    changed_paths, changed_since, feature_runs, only_markdown, push_base, reached_packages,
    reaches, step, workspace_inputs,
};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Which checks a run makes, and over which changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// The quick checks over the changes against `HEAD`, which the pre-commit hook runs.
    Commit,
    /// Every check over what the branch changed since its base, which the pre-push hook runs.
    Push,
    /// Every check on every crate, with each feature pair, whatever changed.
    Full,
}

impl Scope {
    /// Whether the slow checks run: the feature combinations, the cross-target builds, the MSRV checks, and the
    /// board and family crates.
    fn slow(self) -> bool {
        self != Scope::Commit
    }
}

/// Whether a board crate's or family crate's step, by its name, runs for a commit that changes the crate: its
/// format, lints, and builds. Its docs and its MSRV check wait for the push.
fn per_commit(name: &str) -> bool {
    !name.ends_with(" docs") && !name.contains(" MSRV ")
}

/// The paths that `scope` checks. An empty list reaches every group: `Full`, and a push with no base, as before
/// a repo's first push.
fn changed(root: &Path, scope: Scope) -> Vec<PathBuf> {
    match scope {
        Scope::Commit => changed_paths(root),
        Scope::Push => push_base(root)
            .map(|base| changed_since(root, &base))
            .unwrap_or_default(),
        Scope::Full => Vec::new(),
    }
}

/// Runs each check in `scope` in turn and stops at the first failure.
pub fn run(root: &Path, scope: Scope) -> Result<(), String> {
    let full = scope == Scope::Full;
    let msrv = workspace_msrv(root)?;
    let members = members(root)?;
    let targets = toolchain_targets(root)?;
    let boards = boards(root)?;
    let families = families(root)?;
    // Each board crate and family crate builds into its own folder, `check_target`'s.
    let own_targets: Vec<&str> = boards
        .iter()
        .map(|board| board.folder.as_str())
        .chain(families.iter().map(|family| family.folder.as_str()))
        .collect();
    shared::sweep::daily(root, &own_targets);
    let changed = changed(root, scope);

    println!("==> links");
    shared::links::run(root)?;
    println!("==> prose");
    shared::prose::run(root, None)?;
    println!("==> toolchain");
    shared::toolchain::check(root)?;
    if only_markdown(&changed, &[]) {
        println!("==> every cargo check skipped, because the change is only markdown");
        println!("==> every check passed");
        return Ok(());
    }
    shared::require_tool(root, "hack", "cargo install cargo-hack")?;
    step("format", cargo(root).args(["fmt", "--all", "--check"]))?;
    let crates = match reached_packages(root, &changed)? {
        None => vec!["--workspace".to_owned()],
        Some(names) => names
            .into_iter()
            .flat_map(|name| ["--package".to_owned(), name])
            .collect(),
    };
    if crates.is_empty() {
        println!(
            "==> lints and tests skipped, because the change reaches no crate in the workspace"
        );
    } else {
        for (features, args) in feature_runs(full) {
            step(
                &format!("lints, {features}"),
                cargo(root)
                    .args(["hack", "clippy", "--all-targets"])
                    .args(&crates)
                    .args(args)
                    .args(["--", "-D", "warnings"]),
            )?;
        }
        for (features, args) in feature_runs(full) {
            step(
                &format!("tests, {features}"),
                cargo(root).args(["hack", "test"]).args(&crates).args(args),
            )?;
        }
    }
    step(
        "docs",
        cargo(root)
            .args(["doc", "--workspace", "--no-deps"])
            .env("RUSTDOCFLAGS", "-D warnings"),
    )?;
    if full {
        step(
            "feature combinations",
            cargo(root).args([
                "hack",
                "check",
                "--workspace",
                "--feature-powerset",
                "--depth",
                "2",
                "--no-dev-deps",
            ]),
        )?;
    }
    if scope.slow() {
        for (target, args) in cross_builds(&targets, &members) {
            step(&format!("target {target}"), cargo(root).args(args))?;
        }
        for (version, args) in msrv_checks(&msrv, &members) {
            step(&format!("MSRV {version}"), cargo(root).args(args))?;
        }
    } else {
        println!(
            "==> the cross-target builds and the MSRV checks skipped, because the pre-push hook runs them"
        );
    }
    let board_runs = boards
        .iter()
        .map(|board| (board.folder.as_str(), board_checks(board, full)));
    let family_runs = families
        .iter()
        .map(|family| (family.folder.as_str(), family_checks(family, full)));
    for (folder, checks) in board_runs.chain(family_runs) {
        let workspace = folder.split('/').next().unwrap_or(folder);
        if !reaches(&changed, &workspace_inputs(root, workspace)?) {
            println!("==> {folder} skipped, because the change reaches nothing it builds from");
            continue;
        }
        for (name, args) in checks
            .into_iter()
            .filter(|(name, _)| scope.slow() || per_commit(name))
        {
            step(
                &name,
                cargo(&root.join(folder))
                    .args(args)
                    .env("RUSTDOCFLAGS", "-D warnings")
                    .env("CARGO_TARGET_DIR", root.join(check_target(folder))),
            )?;
        }
    }
    println!("==> every check passed");
    Ok(())
}

/// The folder, from the repo root, that one board crate's or family crate's checks build in. The crates of a board
/// workspace set different `ESP_*` configuration in their own `.cargo/config.toml`. Cargo's artifact hash does not
/// include it, so in a shared target folder each crate's checks rebuild esp-hal over the last one's. A family
/// crate's chips build for different targets, which cargo keeps apart in one folder.
fn check_target(folder: &str) -> PathBuf {
    Path::new(folder).join("target")
}

/// A `cargo` command that runs in the workspace root.
pub(crate) fn cargo(root: &Path) -> Command {
    let mut command = Command::new("cargo");
    command.current_dir(root);
    command
}

/// The `rust-version` in the workspace `Cargo.toml`.
fn workspace_msrv(root: &Path) -> Result<String, String> {
    let table = read_toml(&root.join("Cargo.toml"))?;
    table
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("rust-version"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Cargo.toml has no workspace.package.rust-version".to_owned())
}

/// What `cargo xtask check` reads from one workspace member's `Cargo.toml`.
struct Member {
    name: String,
    /// The member's own `rust-version`. `None` means it inherits the workspace's.
    rust_version: Option<String>,
    /// `[package.metadata.devkit] cross = true`: the crate is `no_std` and builds for every target in
    /// `rust-toolchain.toml`.
    cross: bool,
}

impl Member {
    /// The version this member is checked at.
    fn msrv<'a>(&'a self, workspace_msrv: &'a str) -> &'a str {
        self.rust_version.as_deref().unwrap_or(workspace_msrv)
    }
}

/// The workspace members, read from each one's `Cargo.toml`. Each member is listed by its path, because a glob
/// such as `crates/*` is not expanded here.
fn members(root: &Path) -> Result<Vec<Member>, String> {
    let workspace = read_toml(&root.join("Cargo.toml"))?;
    let members = workspace
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .ok_or("Cargo.toml has no workspace.members list")?;
    members
        .iter()
        .filter_map(toml::Value::as_str)
        .map(|member| {
            let path = root.join(member).join("Cargo.toml");
            let table = read_toml(&path)?;
            let package = table
                .get("package")
                .ok_or_else(|| format!("{}: no [package] table", path.display()))?;
            let name = package
                .get("name")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| format!("{}: no package name", path.display()))?;
            let rust_version = package
                .get("rust-version")
                .and_then(toml::Value::as_str)
                .map(str::to_owned);
            let cross = package
                .get("metadata")
                .and_then(|metadata| metadata.get("devkit"))
                .and_then(|devkit| devkit.get("cross"))
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
            Ok(Member {
                name: name.to_owned(),
                rust_version,
                cross,
            })
        })
        .collect()
}

/// The targets listed in `rust-toolchain.toml`. That file is what rustup installs, so it owns the list.
fn toolchain_targets(root: &Path) -> Result<Vec<String>, String> {
    let table = read_toml(&root.join("rust-toolchain.toml"))?;
    let targets = table
        .get("toolchain")
        .and_then(|toolchain| toolchain.get("targets"))
        .and_then(toml::Value::as_array)
        .ok_or("rust-toolchain.toml has no [toolchain] targets list")?;
    Ok(targets
        .iter()
        .filter_map(toml::Value::as_str)
        .map(str::to_owned)
        .collect())
}

/// One `cargo build --target` per target, naming every crate marked `cross`. With no such crate there is
/// nothing to build, and `cargo build` without `--package` would build `xtask` for a microcontroller.
fn cross_builds(targets: &[String], members: &[Member]) -> Vec<(String, Vec<String>)> {
    let cross: Vec<&Member> = members.iter().filter(|member| member.cross).collect();
    if cross.is_empty() {
        return Vec::new();
    }
    targets
        .iter()
        .map(|target| {
            let mut args = vec!["build".to_owned(), "--target".to_owned(), target.clone()];
            for member in &cross {
                args.push("--package".to_owned());
                args.push(member.name.clone());
            }
            (target.clone(), args)
        })
        .collect()
}

/// One `cargo +<version> check` per MSRV, naming every crate that states that version. The versions come out
/// in ascending order.
fn msrv_checks(workspace_msrv: &str, members: &[Member]) -> Vec<(String, Vec<String>)> {
    let mut versions: Vec<&str> = members
        .iter()
        .map(|member| member.msrv(workspace_msrv))
        .collect();
    versions.sort_by_key(|version| version_parts(version));
    versions.dedup();
    versions
        .into_iter()
        .map(|version| {
            let mut args = vec![format!("+{version}"), "check".to_owned()];
            for member in members
                .iter()
                .filter(|member| member.msrv(workspace_msrv) == version)
            {
                args.push("--package".to_owned());
                args.push(member.name.clone());
            }
            args.push("--all-features".to_owned());
            (version.to_owned(), args)
        })
        .collect()
}

/// A board crate's own workspace, which the root workspace excludes because it builds for one microcontroller.
pub(crate) struct Board {
    pub(crate) folder: String,
    rust_version: String,
}

/// The checks run in a board's folder, each named for the board. Its `rust-toolchain.toml` and
/// `.cargo/config.toml` set the target, so no command names one. `cargo test` is not among them, because a
/// board crate's tests run on the board. The target kinds are named, because `--all-targets` also builds the
/// library's own test harness, which needs `std`, even when the library sets `test = false`.
fn board_checks(board: &Board, full: bool) -> Vec<(String, Vec<String>)> {
    let targets = ["--lib", "--bins", "--examples", "--tests"];
    let msrv = format!("+{}", board.rust_version);
    let mut checks: Vec<(String, Vec<&str>)> =
        vec![("format".to_owned(), vec!["fmt", "--all", "--check"])];
    for (features, args) in feature_runs(full) {
        checks.push((
            format!("lints, {features}"),
            [
                &["hack", "clippy"][..],
                &targets,
                &args,
                &["--", "-D", "warnings"],
            ]
            .concat(),
        ));
    }
    for (features, args) in feature_runs(full) {
        checks.push((
            format!("build, {features}"),
            [&["hack", "build"][..], &targets, &args].concat(),
        ));
    }
    checks.push((
        "docs".to_owned(),
        vec!["doc", "--no-deps", "--all-features"],
    ));
    checks.push((
        format!("MSRV {}", board.rust_version),
        vec![&msrv, "check", "--all-features"],
    ));
    checks
        .into_iter()
        .map(|(name, args)| {
            (
                format!("{} {name}", board.folder),
                args.into_iter().map(str::to_owned).collect(),
            )
        })
        .collect()
}

/// The board folders that `[workspace.metadata.devkit] boards` lists, each with the `rust-version` from its own
/// `Cargo.toml`. A board crate is its own workspace, so it states its version itself.
pub(crate) fn boards(root: &Path) -> Result<Vec<Board>, String> {
    let workspace = read_toml(&root.join("Cargo.toml"))?;
    let Some(folders) = workspace
        .get("workspace")
        .and_then(|workspace| workspace.get("metadata"))
        .and_then(|metadata| metadata.get("devkit"))
        .and_then(|devkit| devkit.get("boards"))
        .and_then(toml::Value::as_array)
    else {
        return Ok(Vec::new());
    };
    folders
        .iter()
        .filter_map(toml::Value::as_str)
        .map(|folder| {
            let path = root.join(folder).join("Cargo.toml");
            let rust_version = read_toml(&path)?
                .get("package")
                .and_then(|package| package.get("rust-version"))
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("{}: no package.rust-version", path.display()))?;
            Ok(Board {
                folder: folder.to_owned(),
                rust_version,
            })
        })
        .collect()
}

/// A family crate: a library that several board crates share, in their workspace. It builds only with one chip
/// feature on, for that chip's target.
pub(crate) struct Family {
    folder: String,
    rust_version: String,
    /// `[package.metadata.devkit] chips`: each chip feature, and the target it builds for.
    chips: Vec<(String, String)>,
    /// The crate's other features.
    features: Vec<String>,
}

/// The checks run in a family crate's folder, once for each chip. The chip features exclude each other, so each
/// run turns its chip on and leaves the other chips off. "Every feature" is the chip and the crate's other
/// features, and the powerset excludes the other chips. cargo-hack refuses a feature that is both on and excluded. Only the library builds, because the board crates hold the examples and the tests. There is
/// no format check, because a family crate shares its board crates' workspace, which their checks format.
fn family_checks(family: &Family, full: bool) -> Vec<(String, Vec<String>)> {
    let strings = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    let msrv = format!("+{}", family.rust_version);
    let mut checks = Vec::new();
    for (chip, target) in &family.chips {
        let features = [&[chip.clone()][..], &family.features].concat().join(",");
        let others = family
            .chips
            .iter()
            .map(|(other, _)| other.as_str())
            .filter(|other| other != chip)
            .collect::<Vec<_>>()
            .join(",");
        let name = |check: &str| format!("{} {chip} {check}", family.folder);
        let lints = |features: &[&str]| {
            strings(
                &[
                    &["hack", "clippy", "--lib", "--target", target][..],
                    features,
                    &["--", "-D", "warnings"],
                ]
                .concat(),
            )
        };
        if full {
            let mut pairs = vec!["--feature-powerset", "--depth", "2", "--features", chip];
            if !others.is_empty() {
                pairs.extend(["--exclude-features", &others]);
            }
            checks.push((name("lints, feature pairs"), lints(&pairs)));
        } else {
            checks.push((
                name("lints, every feature"),
                lints(&["--features", &features]),
            ));
            checks.push((
                name("lints, no feature"),
                lints(&["--no-default-features", "--features", chip]),
            ));
        }
        checks.push((
            name("docs"),
            strings(&[
                "doc",
                "--no-deps",
                "--target",
                target,
                "--features",
                &features,
            ]),
        ));
        checks.push((
            name(&format!("MSRV {}", family.rust_version)),
            strings(&[&msrv, "check", "--target", target, "--features", &features]),
        ));
    }
    checks
}

/// The folders that `[workspace.metadata.devkit] families` lists, each read from its own `Cargo.toml`.
pub(crate) fn families(root: &Path) -> Result<Vec<Family>, String> {
    let workspace = read_toml(&root.join("Cargo.toml"))?;
    let Some(folders) = workspace
        .get("workspace")
        .and_then(|workspace| workspace.get("metadata"))
        .and_then(|metadata| metadata.get("devkit"))
        .and_then(|devkit| devkit.get("families"))
        .and_then(toml::Value::as_array)
    else {
        return Ok(Vec::new());
    };
    folders
        .iter()
        .filter_map(toml::Value::as_str)
        .map(|folder| {
            let path = root.join(folder).join("Cargo.toml");
            let manifest = read_toml(&path)?;
            let package = manifest
                .get("package")
                .ok_or_else(|| format!("{}: no [package] table", path.display()))?;
            let rust_version = package
                .get("rust-version")
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("{}: no package.rust-version", path.display()))?;
            let chips: Vec<(String, String)> = package
                .get("metadata")
                .and_then(|metadata| metadata.get("devkit"))
                .and_then(|devkit| devkit.get("chips"))
                .and_then(toml::Value::as_table)
                .ok_or_else(|| format!("{}: no package.metadata.devkit.chips", path.display()))?
                .iter()
                .filter_map(|(chip, target)| Some((chip.clone(), target.as_str()?.to_owned())))
                .collect();
            let features = manifest
                .get("features")
                .and_then(toml::Value::as_table)
                .map(|features| {
                    features
                        .keys()
                        .filter(|feature| chips.iter().all(|(chip, _)| chip != *feature))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            Ok(Family {
                folder: folder.to_owned(),
                rust_version,
                chips,
                features,
            })
        })
        .collect()
}

/// The numbers of a version such as `1.89`, so that `1.100` sorts after `1.89`.
fn version_parts(version: &str) -> Vec<u32> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn read_toml(path: &Path) -> Result<toml::Table, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    text.parse()
        .map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::files::testing::TempRepo;

    #[test]
    fn a_commit_formats_lints_and_builds_a_changed_board_and_leaves_its_docs_and_msrv() {
        let board = Board {
            folder: "esp/embedded-devkit-esp32-c5-devkitc-1".to_owned(),
            rust_version: "1.95".to_owned(),
        };
        let kept: Vec<String> = board_checks(&board, false)
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| per_commit(name))
            .collect();
        let crate_name = "esp/embedded-devkit-esp32-c5-devkitc-1";
        assert_eq!(
            kept,
            [
                "format",
                "lints, every feature",
                "lints, no feature",
                "build, every feature",
                "build, no feature",
            ]
            .map(|step| format!("{crate_name} {step}"))
        );
    }

    #[test]
    fn a_commit_leaves_the_slow_checks_to_the_push_and_the_full_run() {
        assert!(!Scope::Commit.slow());
        assert!(Scope::Push.slow());
        assert!(Scope::Full.slow());
    }

    #[test]
    fn a_commit_checks_its_changes_a_push_the_branch_s_and_a_full_run_everything() {
        let repo = TempRepo::new("check-scope");
        let git = |args: &[&str]| shared::git(&repo.0, args).unwrap();
        git(&["init", "--quiet", "--initial-branch=main"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "test"]);
        repo.write("a.md", "a\n");
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "first"]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.write("b.md", "b\n");
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "second"]);
        repo.write("c.md", "c\n");
        git(&["add", "c.md"]);
        let names = |scope| {
            changed(&repo.0, scope)
                .into_iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(Scope::Commit), ["c.md"]);
        // Without origin/HEAD the push does not have a base, so it checks everything.
        assert_eq!(names(Scope::Push), Vec::<String>::new());
        git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        assert_eq!(names(Scope::Push), ["b.md", "c.md"]);
        assert_eq!(names(Scope::Full), Vec::<String>::new());
    }

    #[test]
    fn each_board_crate_and_family_crate_builds_in_its_own_folder() {
        assert_eq!(
            check_target("esp/embedded-devkit-esp32-c5-devkitc-1"),
            PathBuf::from("esp/embedded-devkit-esp32-c5-devkitc-1/target")
        );
    }

    #[test]
    fn the_workspace_its_members_its_boards_and_its_families_are_read_from_their_manifests() {
        let repo = TempRepo::new("check-manifests");
        for folder in ["a", "b", "esp/board", "esp/family"] {
            std::fs::create_dir_all(repo.0.join(folder)).unwrap();
        }
        repo.write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"a\", \"b\"]\n\n[workspace.package]\nrust-version = \"1.91\"\n\n[workspace.metadata.devkit]\nboards = [\"esp/board\"]\nfamilies = [\"esp/family\"]\n",
        );
        repo.write(
            "rust-toolchain.toml",
            "[toolchain]\ntargets = [\"thumbv6m-none-eabi\", \"riscv32imac-unknown-none-elf\"]\n",
        );
        repo.write(
            "a/Cargo.toml",
            "[package]\nname = \"a\"\nrust-version = \"1.95\"\n\n[package.metadata.devkit]\ncross = true\n",
        );
        repo.write("b/Cargo.toml", "[package]\nname = \"b\"\n");
        repo.write(
            "esp/board/Cargo.toml",
            "[package]\nname = \"board\"\nrust-version = \"1.95\"\n",
        );
        repo.write(
            "esp/family/Cargo.toml",
            "[package]\nname = \"family\"\nrust-version = \"1.95\"\n\n[package.metadata.devkit.chips]\nesp32c5 = \"riscv32imac-unknown-none-elf\"\n\n[features]\nesp32c5 = []\nwifi = []\n",
        );
        assert_eq!(workspace_msrv(&repo.0), Ok("1.91".to_owned()));
        assert_eq!(
            toolchain_targets(&repo.0),
            Ok(vec![
                "thumbv6m-none-eabi".to_owned(),
                "riscv32imac-unknown-none-elf".to_owned(),
            ])
        );
        let members: Vec<(String, Option<String>, bool)> = members(&repo.0)
            .unwrap()
            .into_iter()
            .map(|member| (member.name, member.rust_version, member.cross))
            .collect();
        assert_eq!(
            members,
            [
                ("a".to_owned(), Some("1.95".to_owned()), true),
                ("b".to_owned(), None, false),
            ]
        );
        let boards: Vec<(String, String)> = boards(&repo.0)
            .unwrap()
            .into_iter()
            .map(|board| (board.folder, board.rust_version))
            .collect();
        assert_eq!(boards, [("esp/board".to_owned(), "1.95".to_owned())]);
        let families = families(&repo.0).unwrap();
        assert_eq!(families.len(), 1);
        let family = &families[0];
        assert_eq!(
            (family.folder.as_str(), family.rust_version.as_str()),
            ("esp/family", "1.95")
        );
        assert_eq!(
            family.chips,
            [(
                "esp32c5".to_owned(),
                "riscv32imac-unknown-none-elf".to_owned()
            )]
        );
        assert_eq!(family.features, ["wifi"]);
    }

    fn member(name: &str, rust_version: Option<&str>, cross: bool) -> Member {
        Member {
            name: name.to_owned(),
            rust_version: rust_version.map(str::to_owned),
            cross,
        }
    }

    fn packages(names: &[&str]) -> Vec<String> {
        names
            .iter()
            .flat_map(|name| ["--package", name])
            .map(str::to_owned)
            .collect()
    }

    fn check(version: &str, names: &[&str]) -> (String, Vec<String>) {
        let mut args = vec![format!("+{version}"), "check".to_owned()];
        args.extend(packages(names));
        args.push("--all-features".to_owned());
        (version.to_owned(), args)
    }

    fn build(target: &str, names: &[&str]) -> (String, Vec<String>) {
        let mut args = ["build", "--target", target].map(str::to_owned).to_vec();
        args.extend(packages(names));
        (target.to_owned(), args)
    }

    fn targets() -> Vec<String> {
        ["thumbv6m-none-eabi", "riscv32imac-unknown-none-elf"]
            .map(str::to_owned)
            .to_vec()
    }

    #[test]
    fn crates_are_grouped_by_version_and_an_inherited_version_is_the_workspace_one() {
        let checks = msrv_checks(
            "1.85",
            &[
                member("xtask", None, false),
                member("mcu-run", Some("1.89"), false),
                member("mcu-console", Some("1.85"), true),
            ],
        );
        assert_eq!(
            checks,
            vec![
                check("1.85", &["xtask", "mcu-console"]),
                check("1.89", &["mcu-run"]),
            ]
        );
    }

    #[test]
    fn versions_sort_numerically_not_as_text() {
        let checks = msrv_checks(
            "1.100",
            &[member("a", None, false), member("b", Some("1.89"), false)],
        );
        assert_eq!(checks, vec![check("1.89", &["b"]), check("1.100", &["a"])]);
    }

    #[test]
    fn nothing_is_cross_built_without_a_cross_crate() {
        assert_eq!(
            cross_builds(&targets(), &[member("xtask", None, false)]),
            Vec::new()
        );
    }

    #[test]
    fn a_family_crate_is_linted_documented_and_checked_once_for_each_chip() {
        let family = Family {
            folder: "esp/family".to_owned(),
            rust_version: "1.95".to_owned(),
            chips: vec![
                (
                    "esp32c5".to_owned(),
                    "riscv32imac-unknown-none-elf".to_owned(),
                ),
                (
                    "esp32s31".to_owned(),
                    "riscv32imafc-unknown-none-elf".to_owned(),
                ),
            ],
            features: vec!["embassy".to_owned(), "wifi".to_owned()],
        };
        let chip =
            |chip: &str, target: &str, other: &str, full: bool| -> Vec<(String, Vec<String>)> {
                let every = format!("{chip},embassy,wifi");
                fn lints<'a>(target: &'a str, features: &[&'a str]) -> Vec<&'a str> {
                    [
                        &["hack", "clippy", "--lib", "--target", target][..],
                        features,
                        &["--", "-D", "warnings"],
                    ]
                    .concat()
                }
                let lint_runs = if full {
                    vec![(
                        "lints, feature pairs".to_owned(),
                        lints(
                            target,
                            &[
                                "--feature-powerset",
                                "--depth",
                                "2",
                                "--features",
                                chip,
                                "--exclude-features",
                                other,
                            ],
                        ),
                    )]
                } else {
                    vec![
                        (
                            "lints, every feature".to_owned(),
                            lints(target, &["--features", &every]),
                        ),
                        (
                            "lints, no feature".to_owned(),
                            lints(target, &["--no-default-features", "--features", chip]),
                        ),
                    ]
                };
                lint_runs
                    .into_iter()
                    .chain([
                        (
                            "docs".to_owned(),
                            vec![
                                "doc",
                                "--no-deps",
                                "--target",
                                target,
                                "--features",
                                &format!("{chip},embassy,wifi"),
                            ],
                        ),
                        (
                            "MSRV 1.95".to_owned(),
                            vec![
                                "+1.95",
                                "check",
                                "--target",
                                target,
                                "--features",
                                &format!("{chip},embassy,wifi"),
                            ],
                        ),
                    ])
                    .map(|(name, args)| {
                        (
                            format!("esp/family {chip} {name}"),
                            args.into_iter().map(str::to_owned).collect(),
                        )
                    })
                    .collect()
            };
        for full in [false, true] {
            let mut expected = chip("esp32c5", "riscv32imac-unknown-none-elf", "esp32s31", full);
            expected.extend(chip(
                "esp32s31",
                "riscv32imafc-unknown-none-elf",
                "esp32c5",
                full,
            ));
            assert_eq!(family_checks(&family, full), expected);
        }
    }

    #[test]
    fn a_family_crate_with_one_chip_excludes_no_feature() {
        let family = Family {
            folder: "esp/family".to_owned(),
            rust_version: "1.95".to_owned(),
            chips: vec![(
                "esp32c5".to_owned(),
                "riscv32imac-unknown-none-elf".to_owned(),
            )],
            features: Vec::new(),
        };
        let checks = family_checks(&family, true);
        assert_eq!(
            checks[0].1,
            [
                "hack",
                "clippy",
                "--lib",
                "--target",
                "riscv32imac-unknown-none-elf",
                "--feature-powerset",
                "--depth",
                "2",
                "--features",
                "esp32c5",
                "--",
                "-D",
                "warnings",
            ]
        );
        assert_eq!(checks[1].1[5], "esp32c5");
    }

    #[test]
    fn a_board_is_formatted_linted_built_documented_and_checked_at_its_own_msrv() {
        let board = Board {
            folder: "embedded-devkit-esp32-c5-devkitc-1".to_owned(),
            rust_version: "1.95".to_owned(),
        };
        let targets = ["--lib", "--bins", "--examples", "--tests"];
        let lints = |features: &[&'static str]| {
            [
                &["hack", "clippy"][..],
                &targets,
                features,
                &["--", "-D", "warnings"],
            ]
            .concat()
        };
        let build =
            |features: &[&'static str]| [&["hack", "build"][..], &targets, features].concat();
        let named = |checks: Vec<(&str, Vec<&str>)>| -> Vec<(String, Vec<String>)> {
            checks
                .into_iter()
                .map(|(name, args)| {
                    (
                        format!("embedded-devkit-esp32-c5-devkitc-1 {name}"),
                        args.into_iter().map(str::to_owned).collect(),
                    )
                })
                .collect()
        };
        let format = ("format", vec!["fmt", "--all", "--check"]);
        let docs = ("docs", vec!["doc", "--no-deps", "--all-features"]);
        let msrv = ("MSRV 1.95", vec!["+1.95", "check", "--all-features"]);
        assert_eq!(
            board_checks(&board, false),
            named(vec![
                format.clone(),
                ("lints, every feature", lints(&["--all-features"])),
                ("lints, no feature", lints(&["--no-default-features"])),
                ("build, every feature", build(&["--all-features"])),
                ("build, no feature", build(&["--no-default-features"])),
                docs.clone(),
                msrv.clone(),
            ])
        );
        let powerset = ["--feature-powerset", "--depth", "2"];
        assert_eq!(
            board_checks(&board, true),
            named(vec![
                format,
                ("lints, every feature", lints(&["--all-features"])),
                ("lints, no feature", lints(&["--no-default-features"])),
                ("lints, feature pairs", lints(&powerset)),
                ("build, every feature", build(&["--all-features"])),
                ("build, no feature", build(&["--no-default-features"])),
                ("build, feature pairs", build(&powerset)),
                docs,
                msrv,
            ])
        );
    }

    #[test]
    fn each_target_builds_every_cross_crate_and_no_other() {
        let members = [
            member("xtask", None, false),
            member("a", Some("1.85"), true),
            member("b", None, true),
        ];
        assert_eq!(
            cross_builds(&targets(), &members),
            vec![
                build("thumbv6m-none-eabi", &["a", "b"]),
                build("riscv32imac-unknown-none-elf", &["a", "b"]),
            ]
        );
    }
}
