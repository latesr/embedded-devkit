//! Runs the hardware tests on one board. The board crate's on-target tests run first, because they prove the board
//! works before the scenario tests lean on it.

use crate::check::{boards, cargo};
use crate::shared::checks::step;
use std::path::Path;

/// One command of the hardware tests.
#[derive(Debug, PartialEq, Eq)]
struct Step {
    name: String,
    /// The folder the command runs in: a board folder, or `None` for the workspace root.
    folder: Option<String>,
    args: Vec<String>,
    /// The environment the command reads: `MCU_BOARD` for the scenario tests, and `MCU_DEVICE` for `mcu-run`
    /// and the scenario tests when a device is given.
    env: Vec<(&'static str, String)>,
}

/// Runs each step in turn for the board crate in `board`, and stops at the first failure.
pub fn run(root: &Path, board: Option<&str>, device: Option<&str>) -> Result<(), String> {
    let folders: Vec<String> = boards(root)?
        .into_iter()
        .map(|board| board.folder)
        .collect();
    let folder = pick(&folders, board)?;
    if std::env::var_os("MCU_WIFI_SSID").is_none()
        || std::env::var_os("MCU_WIFI_PASSWORD").is_none()
    {
        println!(
            "==> MCU_WIFI_SSID or MCU_WIFI_PASSWORD is not set, so the Wi-Fi scenario tests test nothing"
        );
    }
    for step_ in steps(&folder, device) {
        let dir = step_
            .folder
            .as_ref()
            .map_or(root.to_path_buf(), |folder| root.join(folder));
        let mut command = cargo(&dir);
        command.args(&step_.args);
        command.envs(step_.env.iter().map(|(key, value)| (key, value)));
        step(&step_.name, &mut command)?;
    }
    println!("==> every hardware test on {folder} passed");
    Ok(())
}

/// The board folder that `board` names, or the only one listed when `board` is `None`. Fails when `board` names
/// no listed folder, or when it is `None` and more than one is listed.
fn pick(folders: &[String], board: Option<&str>) -> Result<String, String> {
    let listed = folders.join(", ");
    match (board, folders) {
        // As paths, so that the separator shell completion adds still matches.
        (Some(board), _) => folders
            .iter()
            .find(|folder| Path::new(folder) == Path::new(board))
            .cloned()
            .ok_or_else(|| format!("{board} is not a listed board crate: {listed}")),
        (None, [only]) => Ok(only.clone()),
        (None, _) => Err(format!(
            "more than one board crate is listed, so name one with --board: {listed}"
        )),
    }
}

/// Build the board's examples and run its on-target tests, both with every feature, because some need one. The
/// scenario tests flash the examples. Then run the scenario tests one at a time, because they share the board.
fn steps(folder: &str, device: Option<&str>) -> Vec<Step> {
    let strings = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    let device: Vec<_> = device
        .map(|device| ("MCU_DEVICE", device.to_owned()))
        .into_iter()
        .collect();
    let mut env = vec![("MCU_BOARD", folder.to_owned())];
    env.extend(device.iter().cloned());
    vec![
        Step {
            name: format!("{folder} examples"),
            folder: Some(folder.to_owned()),
            args: strings(&["build", "--release", "--examples", "--all-features"]),
            env: Vec::new(),
        },
        Step {
            name: format!("{folder} on-target tests"),
            folder: Some(folder.to_owned()),
            args: strings(&["test", "--release", "--all-features"]),
            env: device,
        },
        Step {
            name: "scenario tests".to_owned(),
            folder: None,
            args: strings(&[
                "test",
                "--package",
                "scenario-tests",
                "--",
                "--ignored",
                "--test-threads=1",
            ]),
            env,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn the_board_builds_and_tests_on_target_before_the_scenario_tests() {
        assert_eq!(
            steps("board-a", None),
            [
                Step {
                    name: "board-a examples".to_owned(),
                    folder: Some("board-a".to_owned()),
                    args: strings(&["build", "--release", "--examples", "--all-features"]),
                    env: Vec::new(),
                },
                Step {
                    name: "board-a on-target tests".to_owned(),
                    folder: Some("board-a".to_owned()),
                    args: strings(&["test", "--release", "--all-features"]),
                    env: Vec::new(),
                },
                Step {
                    name: "scenario tests".to_owned(),
                    folder: None,
                    args: strings(&[
                        "test",
                        "--package",
                        "scenario-tests",
                        "--",
                        "--ignored",
                        "--test-threads=1",
                    ]),
                    env: vec![("MCU_BOARD", "board-a".to_owned())],
                },
            ]
        );
    }

    #[test]
    fn a_device_reaches_the_on_target_runner_and_the_scenario_tests() {
        let found = steps("board-a", Some("probe://a"));
        assert_eq!(found[0].env, []);
        assert_eq!(
            found[1].args,
            strings(&["test", "--release", "--all-features"])
        );
        assert_eq!(found[1].env, [("MCU_DEVICE", "probe://a".to_owned())]);
        assert_eq!(
            found[2].env,
            [
                ("MCU_BOARD", "board-a".to_owned()),
                ("MCU_DEVICE", "probe://a".to_owned())
            ]
        );
    }

    #[test]
    fn one_listed_board_is_picked_without_a_name() {
        assert_eq!(pick(&strings(&["board-a"]), None), Ok("board-a".to_owned()));
    }

    #[test]
    fn a_named_board_is_picked_from_several() {
        assert_eq!(
            pick(&strings(&["board-a", "board-b"]), Some("board-b")),
            Ok("board-b".to_owned())
        );
    }

    /// Shell completion ends a folder with a separator.
    #[test]
    fn a_named_board_with_a_trailing_separator_is_picked() {
        assert_eq!(
            pick(
                &strings(&["esp/board-a", "esp/board-b"]),
                Some("esp/board-b/")
            ),
            Ok("esp/board-b".to_owned())
        );
    }

    #[test]
    fn several_listed_boards_need_a_name() {
        assert_eq!(
            pick(&strings(&["board-a", "board-b"]), None),
            Err(
                "more than one board crate is listed, so name one with --board: board-a, board-b"
                    .to_owned()
            )
        );
    }

    #[test]
    fn a_board_that_is_not_listed_is_refused() {
        assert_eq!(
            pick(&strings(&["board-a", "board-b"]), Some("board-c")),
            Err("board-c is not a listed board crate: board-a, board-b".to_owned())
        );
    }
}
