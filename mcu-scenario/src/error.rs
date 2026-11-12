//! Every failure a scenario test can meet before or while it talks to the board.

use std::path::PathBuf;

/// Why a firmware could not be built or started, or its console did not answer.
#[derive(PartialEq, Eq)]
pub enum Error {
    /// `cargo build` for the firmware failed, or could not start.
    Build {
        /// The folder the build ran in.
        dir: PathBuf,
        /// Cargo's own errors, or why cargo did not start.
        message: String,
    },
    /// Cargo built nothing with the firmware's example or binary name.
    NoArtifact {
        /// The example or binary name.
        name: String,
        /// The folder the build ran in.
        dir: PathBuf,
    },
    /// `mcu-run` failed: no board, the probe, the flash, the console, or the protocol version.
    Run(mcu_run::Error),
    /// [`host_address`](crate::host_address) found no single interface of this host on the board's subnet, or
    /// could not list the interfaces. The text says which.
    HostAddress(String),
    /// [`Console::send_until`](crate::Console::send_until) took no value from any reply within the wait.
    NotReached {
        /// The line that was sent.
        line: String,
        /// How long the wait was.
        waited: std::time::Duration,
        /// The last reply, or `None` when no send was made.
        last: Option<mcu_run::Reply>,
    },
}

impl From<mcu_run::Error> for Error {
    fn from(error: mcu_run::Error) -> Self {
        Error::Run(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Build { dir, message } => {
                write!(
                    f,
                    "the firmware in {} did not build:\n{message}",
                    dir.display()
                )
            }
            Error::NoArtifact { name, dir } => write!(
                f,
                "cargo in {} built no example or binary named {name}",
                dir.display()
            ),
            Error::Run(error) => write!(f, "{error}"),
            Error::HostAddress(error) => write!(f, "this host's address: {error}"),
            Error::NotReached { line, waited, last } => {
                write!(
                    f,
                    "{line} did not give the awaited reply within {waited:?}."
                )?;
                let Some(last) = last else {
                    return write!(f, " Nothing was sent.");
                };
                write!(f, " The last reply:")?;
                for body in &last.body {
                    write!(f, "\n  {body}")?;
                }
                match &last.result {
                    mcu_run::CommandResult::Ok => write!(f, "\n  ok"),
                    mcu_run::CommandResult::Err(reason) => write!(f, "\n  err {reason}"),
                }
            }
        }
    }
}

/// Writes the [`Display`](std::fmt::Display) text, so `.unwrap()` in a test shows cargo's errors line by line.
impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_error_names_what_failed_and_where() {
        let build = Error::Build {
            dir: PathBuf::from("fw"),
            message: "error[E0425]: cannot find value".to_owned(),
        };
        assert_eq!(
            build.to_string(),
            "the firmware in fw did not build:\nerror[E0425]: cannot find value"
        );
        let none = Error::NoArtifact {
            name: "console".to_owned(),
            dir: PathBuf::from("fw"),
        };
        assert_eq!(
            none.to_string(),
            "cargo in fw built no example or binary named console"
        );
        let run = Error::from(mcu_run::Error::Probe("no probe".to_owned()));
        assert_eq!(run.to_string(), "probe: no probe");
    }

    #[test]
    fn not_reached_gives_an_err_result_and_says_when_nothing_was_sent() {
        let err = Error::NotReached {
            line: "net config".to_owned(),
            waited: std::time::Duration::from_secs(20),
            last: Some(mcu_run::Reply {
                body: vec![],
                result: mcu_run::CommandResult::Err("no link".to_owned()),
            }),
        };
        assert_eq!(
            err.to_string(),
            "net config did not give the awaited reply within 20s. The last reply:\n  err no link"
        );
        let none = Error::NotReached {
            line: "net config".to_owned(),
            waited: std::time::Duration::ZERO,
            last: None,
        };
        assert_eq!(
            none.to_string(),
            "net config did not give the awaited reply within 0ns. Nothing was sent."
        );
    }

    #[test]
    fn debug_is_the_display_text_with_real_line_ends() {
        let build = Error::Build {
            dir: PathBuf::from("fw"),
            message: "error[E0425]: cannot find value\n  --> src/main.rs:3:5".to_owned(),
        };
        assert_eq!(
            format!("{build:?}"),
            "the firmware in fw did not build:\nerror[E0425]: cannot find value\n  --> src/main.rs:3:5"
        );
    }
}
