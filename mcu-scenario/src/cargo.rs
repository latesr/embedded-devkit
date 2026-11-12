//! Builds a firmware with cargo, and finds its ELF in cargo's JSON messages.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What cargo builds: an example or a binary target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Target {
    Example,
    Bin,
}

impl Target {
    /// The option that names the target, and the kind in cargo's messages.
    fn kind(self) -> &'static str {
        match self {
            Target::Example => "example",
            Target::Bin => "bin",
        }
    }
}

/// The release build of `target` `name` in `dir`, with `features`, that writes its messages as JSON.
pub(crate) fn command(dir: &Path, target: Target, name: &str, features: &[&str]) -> Command {
    let mut command = Command::new("cargo");
    command
        .current_dir(dir)
        .args([
            "build",
            "--release",
            "--message-format=json-render-diagnostics",
        ])
        .arg(format!("--{}", target.kind()))
        .arg(name);
    if !features.is_empty() {
        command.arg("--features").arg(features.join(","));
    }
    // `cargo test` names the host's toolchain in RUSTUP_TOOLCHAIN, which would override the firmware's
    // rust-toolchain.toml.
    command.env_remove("RUSTUP_TOOLCHAIN");
    command
}

/// Copies each line of `from` to `to` as it arrives, and returns every line. A write to `to` that fails does not
/// stop the copy, so the returned text stays whole.
pub(crate) fn tee(from: impl std::io::Read, mut to: impl std::io::Write) -> String {
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(from);
    let mut kept = Vec::new();
    let mut line = Vec::new();
    while reader
        .read_until(b'\n', &mut line)
        .is_ok_and(|read| read > 0)
    {
        let _ = to.write_all(&line).and_then(|()| to.flush());
        kept.append(&mut line);
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// The ELF that `stdout`, cargo's JSON messages, names for `target` `name`.
pub(crate) fn artifact(stdout: &str, target: Target, name: &str) -> Option<PathBuf> {
    stdout.lines().find_map(|line| {
        let message: serde_json::Value = serde_json::from_str(line).ok()?;
        let built = message.get("target")?;
        let kind_matches = built
            .get("kind")?
            .as_array()?
            .iter()
            .any(|kind| kind == target.kind());
        let is_artifact = message.get("reason")? == "compiler-artifact";
        if !is_artifact || !kind_matches || built.get("name")? != name {
            return None;
        }
        message.get("executable")?.as_str().map(PathBuf::from)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn args(command: &Command) -> Vec<&str> {
        command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect()
    }

    #[test]
    fn the_build_is_a_release_build_of_the_target_with_json_messages_in_its_folder() {
        let example = command(Path::new("fw"), Target::Example, "console", &[]);
        assert_eq!(example.get_program(), OsStr::new("cargo"));
        assert_eq!(
            args(&example),
            [
                "build",
                "--release",
                "--message-format=json-render-diagnostics",
                "--example",
                "console"
            ]
        );
        assert_eq!(example.get_current_dir(), Some(Path::new("fw")));
        let bin = command(Path::new("fw"), Target::Bin, "app", &["wifi", "iperf"]);
        assert_eq!(
            args(&bin),
            [
                "build",
                "--release",
                "--message-format=json-render-diagnostics",
                "--bin",
                "app",
                "--features",
                "wifi,iperf"
            ]
        );
    }

    #[test]
    fn the_build_takes_the_firmware_s_toolchain_not_the_test_s() {
        let build = command(Path::new("fw"), Target::Example, "console", &[]);
        let removed: Vec<_> = build
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name)
            .collect();
        assert_eq!(removed, [OsStr::new("RUSTUP_TOOLCHAIN")]);
    }

    /// A pipe that gives `data`, then its end, and fails the test at a third read of the end. A read can meet the
    /// end twice when the last line does not end in a line end. A copy that does not stop at the end then fails instead of
    /// hanging.
    struct Pipe {
        data: &'static [u8],
        ends: u8,
    }

    fn pipe(data: &'static str) -> Pipe {
        Pipe {
            data: data.as_bytes(),
            ends: 0,
        }
    }

    impl std::io::Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.data.is_empty() {
                self.ends = self.ends.saturating_add(1);
                assert!(self.ends <= 2, "read again after the end");
                return Ok(0);
            }
            let n = buf.len().min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    #[test]
    fn tee_copies_each_line_and_keeps_all_of_them() {
        let mut copy = Vec::new();
        let kept = tee(
            pipe("   Compiling esp-hal\nerror[E0425]: cannot find value\nno line end"),
            &mut copy,
        );
        assert_eq!(
            kept,
            "   Compiling esp-hal\nerror[E0425]: cannot find value\nno line end"
        );
        assert_eq!(String::from_utf8(copy).unwrap(), kept);
    }

    #[test]
    fn tee_keeps_the_text_when_the_copy_fails() {
        struct Broken;
        impl std::io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("closed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(tee(pipe("a\nb\n"), Broken), "a\nb\n");
    }

    const MESSAGES: &str = r#"{"reason":"compiler-artifact","target":{"kind":["custom-build"],"name":"build-script-build"},"executable":null}
not json, such as a line a build script printed
{"reason":"compiler-artifact","target":{"kind":["lib"],"name":"console"},"executable":null}
{"reason":"compiler-artifact","target":{"kind":["bin"],"name":"console"},"executable":"/t/release/console"}
{"reason":"compiler-artifact","target":{"kind":["example"],"name":"other"},"executable":"/t/release/examples/other"}
{"reason":"compiler-artifact","target":{"kind":["example"],"name":"console"},"executable":"/t/release/examples/console"}
{"reason":"build-finished","success":true}
"#;

    #[test]
    fn the_artifact_is_the_executable_of_the_target_with_that_kind_and_name() {
        assert_eq!(
            artifact(MESSAGES, Target::Example, "console"),
            Some(PathBuf::from("/t/release/examples/console"))
        );
        assert_eq!(
            artifact(MESSAGES, Target::Bin, "console"),
            Some(PathBuf::from("/t/release/console"))
        );
        assert_eq!(artifact(MESSAGES, Target::Example, "missing"), None);
        assert_eq!(artifact(MESSAGES, Target::Bin, "other"), None);
        assert_eq!(artifact("", Target::Example, "console"), None);
    }
}
