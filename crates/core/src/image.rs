//! Where an agent's image comes from, for the gateway that starts it.
//!
//! A gateway on this Mac shares its Docker, so an image built here is one it can start. A gateway in a cluster can't
//! pull from this Mac, so its image is pushed to a registry it can reach, or is already there.

use crate::agent::Step;
use crate::docker::{Docker, DockerError};

/// The CPU an image is built for. It's not always the Mac's: Apple Silicon builds `linux/arm64`, and a cluster is often
/// `linux/amd64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Arm64,
    Amd64,
}

impl Platform {
    /// The Mac's own, which Docker on a Mac builds for and runs.
    pub fn host() -> Platform {
        if cfg!(target_arch = "aarch64") {
            Platform::Arm64
        } else {
            Platform::Amd64
        }
    }

    /// As Docker's `--platform` names it.
    pub fn docker(self) -> &'static str {
        match self {
            Platform::Arm64 => "linux/arm64",
            Platform::Amd64 => "linux/amd64",
        }
    }
}

/// An image to have ready: what to build it from, and the name it has on this Mac.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub tag: String,
    pub dockerfile: &'static str,
    pub args: Vec<(&'static str, &'static str)>,
}

/// Somewhere to get an agent's image from, which makes it ready and says what to start a sandbox from.
pub trait ImageSource {
    /// The platform its images are for, which an agent's policy has to be written for too.
    fn platform(&self) -> Platform;

    /// Makes `image` ready where the gateway can start it, saying each step to `step` as it starts, and gives the
    /// reference to start a sandbox from.
    fn ready(&self, image: &Image, step: &dyn Fn(Step)) -> Result<String, DockerError>;

    /// This Mac's Docker, when the gateway starts the images it builds, which a kit's install script is run in. A source
    /// whose images go to a registry has none.
    fn docker(&self) -> Option<&Docker> {
        None
    }
}

/// `tag` under a registry's `prefix`: `registry.example.com/team/playpen-codex:0.171.0`.
fn reference(prefix: &str, tag: &str) -> String {
    format!("{}/{tag}", prefix.trim_end_matches('/'))
}

/// This Mac's Docker, for a gateway that shares it: the image is built when it isn't there, and has its own name.
impl ImageSource for Docker {
    fn platform(&self) -> Platform {
        Platform::host()
    }

    fn ready(&self, image: &Image, step: &dyn Fn(Step)) -> Result<String, DockerError> {
        if !self.has_image(&image.tag)? {
            step(Step::Building);
            self.build(&image.tag, image.dockerfile, &image.args, None)?;
        }
        Ok(image.tag.clone())
    }

    fn docker(&self) -> Option<&Docker> {
        Some(self)
    }
}

/// A registry the gateway can pull from: the image is built here for `platform` and pushed under `prefix`, unless it's
/// there already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    docker: Docker,
    prefix: String,
    platform: Platform,
}

impl Registry {
    pub fn new(docker: Docker, prefix: impl Into<String>, platform: Platform) -> Registry {
        Registry {
            docker,
            prefix: prefix.into(),
            platform,
        }
    }
}

impl ImageSource for Registry {
    fn platform(&self) -> Platform {
        self.platform
    }

    fn ready(&self, image: &Image, step: &dyn Fn(Step)) -> Result<String, DockerError> {
        let reference = reference(&self.prefix, &image.tag);
        if self.docker.has_remote_image(&reference) {
            return Ok(reference);
        }
        step(Step::Building);
        self.docker.build(
            &reference,
            image.dockerfile,
            &image.args,
            Some(self.platform),
        )?;
        step(Step::Pushing);
        self.docker.push(&reference)?;
        Ok(reference)
    }
}

/// An image the person provides, for a gateway Playpen can't push to: it only names the image, under `prefix`, and
/// builds and pushes nothing. OpenShell says so when the gateway can't find it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provided {
    prefix: String,
    platform: Platform,
}

impl Provided {
    pub fn new(prefix: impl Into<String>, platform: Platform) -> Provided {
        Provided {
            prefix: prefix.into(),
            platform,
        }
    }
}

impl ImageSource for Provided {
    fn platform(&self) -> Platform {
        self.platform
    }

    fn ready(&self, image: &Image, _step: &dyn Fn(Step)) -> Result<String, DockerError> {
        Ok(reference(&self.prefix, &image.tag))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::{Image, ImageSource, Platform, Provided, reference};

    fn image() -> Image {
        Image {
            tag: "playpen-codex:0.171.0".to_string(),
            dockerfile: "FROM scratch\n",
            args: vec![],
        }
    }

    #[test]
    fn an_image_is_named_under_the_registry_it_is_pushed_to() {
        assert_eq!(
            reference("registry.example.com/team", "playpen-codex:0.171.0"),
            "registry.example.com/team/playpen-codex:0.171.0"
        );
        assert_eq!(
            reference("registry.example.com/team/", "playpen-codex:0.171.0"),
            "registry.example.com/team/playpen-codex:0.171.0",
            "a slash on the end of the prefix isn't doubled"
        );
    }

    #[test]
    fn an_image_the_person_provides_is_named_and_nothing_is_built() {
        let steps = RefCell::new(Vec::new());
        let provided = Provided::new("ghcr.io/phin-tech", Platform::Amd64);
        let made = provided
            .ready(&image(), &|step| steps.borrow_mut().push(step))
            .unwrap();
        assert_eq!(made, "ghcr.io/phin-tech/playpen-codex:0.171.0");
        assert!(steps.borrow().is_empty());
        assert_eq!(provided.platform(), Platform::Amd64);
    }

    #[test]
    fn a_platform_is_named_as_docker_names_it() {
        assert_eq!(Platform::Arm64.docker(), "linux/arm64");
        assert_eq!(Platform::Amd64.docker(), "linux/amd64");
    }
}
