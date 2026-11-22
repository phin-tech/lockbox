//! The `playpen` command. It's the app's own binary: given a command it knows, `main` runs it here and exits
//! before any window opens. Anything else opens the app, since macOS starts apps with arguments of its own.

use std::io::{self, Write};
use std::path::PathBuf;

use playpen_core::{ServiceCatalog, version};

/// What a command line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Version,
    Help,
    ServicesList(String),
    ServicesExport(Vec<String>),
    ServicesImport(PathBuf),
    Wrong(String),
}

const USAGE: &str = "usage: playpen [command]

  services [query]          list or search the bundled common services catalog
  services export [id…]     print shareable services catalog JSON
  services import <file>    load a shared services catalog JSON into Playpen
  --version, --help

With no arguments, opens Playpen.";

/// The command `args` (without the program's name) ask for, or none, to open the app.
pub fn parse<S: AsRef<str>>(args: &[S]) -> Option<Command> {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    match *args.first()? {
        "--version" | "-v" | "version" => Some(Command::Version),
        "--help" | "-h" | "help" => Some(Command::Help),
        "services" | "catalog" => Some(match args.get(1).copied() {
            None => Command::ServicesList(String::new()),
            Some("list" | "ls") => Command::ServicesList(args[2..].join(" ")),
            Some("export") => {
                Command::ServicesExport(args[2..].iter().map(|id| (*id).to_string()).collect())
            }
            Some("import") => match args.get(2) {
                Some(path) if args.len() == 3 => Command::ServicesImport(PathBuf::from(path)),
                Some(_) => Command::Wrong("services import takes one file at a time".to_string()),
                None => Command::Wrong("give the path to the services JSON file".to_string()),
            },
            Some(flag) if flag.starts_with('-') => {
                Command::Wrong(format!("services doesn't take {flag}"))
            }
            Some(_) => Command::ServicesList(args[1..].join(" ")),
        }),
        _ => None,
    }
}

fn data_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PLAYPEN_SETTINGS") {
        return PathBuf::from(path).parent().map(PathBuf::from);
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(home.join("Library/Application Support/tech.phin.playpen"))
}

impl Command {
    /// Runs the command, writing to `out`, and gives the exit status.
    pub fn run(self, out: &mut impl Write) -> io::Result<i32> {
        match self {
            Command::Version => writeln!(out, "playpen {}", version())?,
            Command::Help => writeln!(out, "{USAGE}")?,
            Command::Wrong(reason) => {
                writeln!(out, "playpen: {reason}\n\n{USAGE}")?;
                return Ok(2);
            }
            Command::ServicesList(query) => {
                let dir = data_dir();
                let (catalog, errors) = ServiceCatalog::load(dir.as_deref());
                for error in errors {
                    writeln!(out, "warning: {error}")?;
                }
                let shown = catalog.filter(&query, None);
                if shown.is_empty() {
                    writeln!(out, "No services match “{query}”.")?;
                } else {
                    for service in shown {
                        writeln!(
                            out,
                            "  {:<16} {:<30} {:<26} {}",
                            service.id,
                            service.resolved_host(),
                            service.variable,
                            service.summary
                        )?;
                    }
                }
            }
            Command::ServicesExport(ids) => {
                let dir = data_dir();
                let (catalog, _) = ServiceCatalog::load(dir.as_deref());
                writeln!(out, "{}", catalog.export_json(&ids))?;
            }
            Command::ServicesImport(file) => {
                let Some(dir) = data_dir() else {
                    writeln!(out, "playpen: couldn't locate Playpen's settings folder")?;
                    return Ok(1);
                };
                match ServiceCatalog::import_file(&dir, &file) {
                    Ok((stored, count)) => writeln!(
                        out,
                        "Imported {count} service{} into {}.",
                        if count == 1 { "" } else { "s" },
                        dir.join("services").join(stored).display()
                    )?,
                    Err(error) => {
                        writeln!(out, "playpen: {error}")?;
                        return Ok(1);
                    }
                }
            }
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
            format!("playpen {}", playpen_core::version())
        );
    }

    #[test]
    fn help_says_how_to_use_it() {
        assert_eq!(parse(&["-h"]), Some(Command::Help));
        assert!(output(Command::Help).starts_with("usage: playpen"));
    }

    #[test]
    fn services_commands_list_and_export_the_bundled_catalog() {
        assert_eq!(
            parse(&["services", "statsig"]),
            Some(Command::ServicesList("statsig".to_string()))
        );
        let listed = output(Command::ServicesList("statsig".to_string()));
        assert!(listed.contains("statsigapi.net"), "{listed}");
        let exported = output(Command::ServicesExport(vec!["patchbay".to_string()]));
        let parsed = playpen_core::ServiceCatalogFile::from_json(&exported).unwrap();
        assert_eq!(parsed.services.len(), 1);
        assert_eq!(parsed.services[0].id, "patchbay");
    }

    #[test]
    fn anything_else_opens_the_app() {
        assert_eq!(parse::<&str>(&[]), None);
        assert_eq!(parse(&["-psn_0_12345"]), None);
        assert_eq!(parse(&["-NSDocumentRevisionsDebugMode", "YES"]), None);
        assert_eq!(parse(&["notes.txt"]), None);
    }
}
