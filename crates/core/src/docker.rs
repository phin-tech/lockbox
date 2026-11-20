//! Docker, through its `docker` command, for building the images agents run in. OpenShell's gateway on a Mac runs
//! sandboxes in the same Docker (OrbStack's or Docker Desktop's), so an image built here is one it can start.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

    /// Builds `dockerfile` as the image `tag`, with `args` as its build arguments. It needs no files beside it.
    pub fn build(
        &self,
        tag: &str,
        dockerfile: &str,
        args: &[(&str, &str)],
    ) -> Result<(), DockerError> {
        let mut command = self.command();
        command.args(["build", "--quiet", "--tag", tag]);
        for (name, value) in args {
            command.args(["--build-arg", &format!("{name}={value}")]);
        }
        let mut child = command
            .arg("-")
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
        let said = String::from_utf8_lossy(&output.stderr);
        let last = said
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("it gave no reason");
        Err(DockerError::Build(tag.to_string(), last.trim().to_string()))
    }
}
