//! Docker, through its `docker` command, for building the images agents run in. OpenShell's gateway on a Mac runs
//! sandboxes in the same Docker (OrbStack's or Docker Desktop's), so an image built here is one it can start.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::image::Platform;

/// Why Docker couldn't do what was asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DockerError {
    #[error(
        "Docker isn't installed. OpenShell runs sandboxes in Docker, so install OrbStack or Docker Desktop first."
    )]
    NotInstalled,
    #[error("Couldn't run Docker: {0}")]
    Run(String),
    #[error("Couldn't build {0}: {1}")]
    Build(String, String),
    #[error("Couldn't push {0}: {1}")]
    Push(String, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Docker {
    binary: PathBuf,
}

impl Docker {
    /// The installed `docker`, looked for by its full path first, since an app opened from the Finder has no shell's
    /// `PATH`.
    pub fn find() -> Result<Docker, DockerError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut places = vec![
            PathBuf::from("/usr/local/bin/docker"),
            PathBuf::from("/opt/homebrew/bin/docker"),
            PathBuf::from("/Applications/Docker.app/Contents/Resources/bin/docker"),
        ];
        if let Some(home) = home {
            places.push(home.join(".orbstack/bin/docker"));
        }
        if let Some(path) = std::env::var_os("PATH") {
            places.extend(std::env::split_paths(&path).map(|dir| dir.join("docker")));
        }
        places
            .into_iter()
            .find(|place| place.is_file())
            .map(|binary| Docker { binary })
            .ok_or(DockerError::NotInstalled)
    }

    /// The `docker` at `binary`, for a test that never runs it.
    #[cfg(test)]
    pub(crate) fn at(binary: impl Into<PathBuf>) -> Docker {
        Docker {
            binary: binary.into(),
        }
    }

    /// A `PATH` with the docker binary's own folder in it, where its credential helpers are, and Homebrew's.
    fn path(&self) -> OsString {
        let mut dirs: Vec<PathBuf> = self
            .binary
            .parent()
            .map(Path::to_path_buf)
            .into_iter()
            .collect();
        dirs.extend(["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin", "/bin"].map(PathBuf::from));
        std::env::join_paths(dirs).unwrap_or_default()
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        command.env("PATH", self.path());
        command
    }

    /// Whether the image `tag` is already built.
    pub fn has_image(&self, tag: &str) -> Result<bool, DockerError> {
        let status = self
            .command()
            .args(["image", "inspect", tag])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| DockerError::Run(error.to_string()))?;
        Ok(status.success())
    }

    /// Builds the folder `context`, whose `Dockerfile` copies files from beside it, as the image `tag`.
    pub fn build_folder(&self, tag: &str, context: &Path) -> Result<(), DockerError> {
        let output = self
            .command()
            .args(["build", "--quiet", "--tag", tag])
            .arg(context)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| DockerError::Run(error.to_string()))?;
        if output.status.success() {
            return Ok(());
        }
        // The last few lines, since a setup script's own error comes just before Docker's.
        let said = String::from_utf8_lossy(&output.stderr);
        let lines: Vec<&str> = said
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        let tail = lines[lines.len().saturating_sub(4)..].join("\n");
        Err(DockerError::Build(tag.to_string(), tail))
    }

    /// Whether the image `reference` is in its registry. A registry that can't be reached, or won't say, counts as no.
    pub fn has_remote_image(&self, reference: &str) -> bool {
        self.command()
            .args(["manifest", "inspect", reference])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Builds `dockerfile` as the image `tag`, with `args` as its build arguments, for `platform` when given and the
    /// Mac's otherwise. It needs no files beside it.
    pub fn build(
        &self,
        tag: &str,
        dockerfile: &str,
        args: &[(&str, &str)],
        platform: Option<Platform>,
    ) -> Result<(), DockerError> {
        let mut child = self
            .command()
            .args(build_arguments(tag, args, platform))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| DockerError::Run(error.to_string()))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(dockerfile.as_bytes())
                .map_err(|error| DockerError::Run(error.to_string()))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| DockerError::Run(error.to_string()))?;
        if output.status.success() {
            return Ok(());
        }
        Err(DockerError::Build(
            tag.to_string(),
            last_line(&output.stderr),
        ))
    }

    /// Pushes the image `reference` to the registry its name says.
    pub fn push(&self, reference: &str) -> Result<(), DockerError> {
        let output = self
            .command()
            .args(["push", "--quiet", reference])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| DockerError::Run(error.to_string()))?;
        if output.status.success() {
            return Ok(());
        }
        Err(DockerError::Push(
            reference.to_string(),
            last_line(&output.stderr),
        ))
    }
}

/// What follows `docker` to build `tag` from a Dockerfile on its standard input.
fn build_arguments(tag: &str, args: &[(&str, &str)], platform: Option<Platform>) -> Vec<String> {
    let mut all: Vec<String> = ["build", "--quiet", "--tag", tag].map(String::from).into();
    if let Some(platform) = platform {
        all.extend(["--platform".to_string(), platform.docker().to_string()]);
    }
    for (name, value) in args {
        all.extend(["--build-arg".to_string(), format!("{name}={value}")]);
    }
    all.push("-".to_string());
    all
}

/// The last line Docker said, which is where it says why it failed.
fn last_line(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("it gave no reason")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::build_arguments;
    use crate::image::Platform;

    #[test]
    fn a_build_for_this_mac_names_no_platform() {
        assert_eq!(
            build_arguments("playpen-codex:1", &[("CODEX_VERSION", "1")], None),
            [
                "build",
                "--quiet",
                "--tag",
                "playpen-codex:1",
                "--build-arg",
                "CODEX_VERSION=1",
                "-"
            ]
        );
    }

    #[test]
    fn a_build_for_a_cluster_names_its_platform_before_the_build_arguments() {
        let args = build_arguments(
            "registry.example.com/playpen-codex:1",
            &[("CODEX_VERSION", "1")],
            Some(Platform::Amd64),
        );
        assert_eq!(
            args[4..],
            [
                "--platform",
                "linux/amd64",
                "--build-arg",
                "CODEX_VERSION=1",
                "-"
            ]
        );
    }
}
