//! The `lockbox` command. It's the app's own binary: given a command it knows, `main` runs it here and exits
//! before any window opens. Anything else opens the app, since macOS starts apps with arguments of its own.

use std::io::{self, Write};

use lockbox_core::version;

/// What a command line asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Version,
    Help,
}

const USAGE: &str = "usage: lockbox [--version | --help]\n\nWith no arguments, opens Lockbox.";

/// The command `args` (without the program's name) ask for, or none, to open the app.
pub fn parse<S: AsRef<str>>(args: &[S]) -> Option<Command> {
    match args.first()?.as_ref() {
        "--version" | "-v" | "version" => Some(Command::Version),
        "--help" | "-h" | "help" => Some(Command::Help),
        _ => None,
    }
}

impl Command {
    /// Runs the command, writing to `out`, and gives the exit status.
    pub fn run(self, out: &mut impl Write) -> io::Result<i32> {
        match self {
            Command::Version => writeln!(out, "lockbox {}", version())?,
            Command::Help => writeln!(out, "{USAGE}")?,
        }
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, parse};

    fn output(command: Command) -> String {
        let mut out = Vec::new();
        assert_eq!(command.run(&mut out).unwrap(), 0);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn version_prints_the_version_and_build() {
        assert_eq!(parse(&["--version"]), Some(Command::Version));
        let printed = output(Command::Version);
        assert_eq!(
            printed.trim(),
            format!("lockbox {}", lockbox_core::version())
        );
    }

    #[test]
    fn help_says_how_to_use_it() {
        assert_eq!(parse(&["-h"]), Some(Command::Help));
        assert!(output(Command::Help).starts_with("usage: lockbox"));
    }

    #[test]
    fn anything_else_opens_the_app() {
        assert_eq!(parse::<&str>(&[]), None);
        assert_eq!(parse(&["-psn_0_12345"]), None);
        assert_eq!(parse(&["-NSDocumentRevisionsDebugMode", "YES"]), None);
        assert_eq!(parse(&["notes.txt"]), None);
    }
}
