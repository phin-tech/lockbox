//! Kits' setup folders, like dotfiles repositories: each folder's `install.sh`, if it has one, runs as root while the
//! sandbox's image is built on the Mac, where Docker reaches the internet, installing whatever it likes however it
//! likes; and the folder's dotfiles are linked into each new sandbox's home. Images are named by what went into them,
//! so one is built once and used until a kit's folder changes.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};

use crate::agent::Agent;
use crate::docker::{Docker, DockerError};
use crate::openshell::{OpenShell, OpenShellError};

/// Files that usually hold secrets, which are never linked into a sandbox: a secret goes in as a service or a kit
/// variable from 1Password or the Keychain.
const SECRET_FILES: [&str; 7] = [
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".ssh",
    ".aws",
    ".docker",
];

/// A kit's setup folder, by the kit's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupFolder {
    pub kit: String,
    pub folder: PathBuf,
}

impl SetupFolder {
    fn installs(&self) -> bool {
        self.folder.join("install.sh").is_file()
    }

    /// A name for its folder in an image or a sandbox: the kit's, kept to what a path takes plainly.
    fn slug(&self) -> String {
        self.kit
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    }
}

/// The files in `folder`, by path, leaving out `.git`, for hashing what an image is built from.
fn files(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut waiting = vec![folder.to_path_buf()];
    while let Some(dir) = waiting.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name() == ".git" {
                continue;
            }
            if path.is_dir() {
                waiting.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The image for `agent` with the setup folders' `install.sh`s run over it, in order, or the agent's own when none
/// installs anything. A folder whose files changed makes another.
pub fn image_name(agent: Agent, setups: &[SetupFolder]) -> String {
    let installing: Vec<&SetupFolder> = setups.iter().filter(|setup| setup.installs()).collect();
    if installing.is_empty() {
        return agent.image();
    }
    let mut hasher = DefaultHasher::new();
    agent.image().hash(&mut hasher);
    for setup in &installing {
        setup.slug().hash(&mut hasher);
        for file in files(&setup.folder) {
            file.strip_prefix(&setup.folder)
                .unwrap_or(&file)
                .hash(&mut hasher);
            std::fs::read(&file).unwrap_or_default().hash(&mut hasher);
        }
    }
    format!("playpen-{}-kits:{:016x}", agent.id(), hasher.finish())
}

/// The Dockerfile that runs each folder's `install.sh` over the agent's image.
fn dockerfile(agent: Agent, setups: &[&SetupFolder]) -> String {
    let mut text = format!(
        "# Written by Playpen: kits' install.sh, run over {image}.\nFROM {image}\n",
        image = agent.image()
    );
    for (at, setup) in setups.iter().enumerate() {
        let into = format!("/opt/playpen/kits/{}", setup.slug());
        text.push_str(&format!(
            "COPY kit-{at} {into}\nRUN cd {into} && sh ./install.sh\n"
        ));
    }
    text
}

/// Builds the image [`image_name`] names, if it isn't built yet: the agent's own, then the folders' `install.sh`s.
pub fn build(agent: Agent, setups: &[SetupFolder], docker: &Docker) -> Result<String, DockerError> {
    let image = image_name(agent, setups);
    if docker.has_image(&image)? {
        return Ok(image);
    }
    if !docker.has_image(&agent.image())? {
        agent.build_image(docker)?;
    }
    if image == agent.image() {
        return Ok(image);
    }
    let installing: Vec<&SetupFolder> = setups.iter().filter(|setup| setup.installs()).collect();
    let context = std::env::temp_dir().join(format!("playpen-kits-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&context);
    let copied = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&context)?;
        for (at, setup) in installing.iter().enumerate() {
            let into = context.join(format!("kit-{at}"));
            for file in files(&setup.folder) {
                let target = into.join(file.strip_prefix(&setup.folder).unwrap_or(&file));
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&file, &target)?;
            }
        }
        std::fs::write(context.join("Dockerfile"), dockerfile(agent, &installing))
    })();
    let built = copied
        .map_err(|error| DockerError::Build(image.clone(), error.to_string()))
        .and_then(|()| docker.build_folder(&image, &context));
    let _ = std::fs::remove_dir_all(&context);
    built.map(|()| image)
}

/// What links a setup folder's dotfiles into the sandbox's home, leaving alone any that's there already and any that
/// usually holds a secret.
const LINK: &str = "set -e
dir=\"$1\"; shift
for path in \"$dir\"/.[!.]* \"$dir\"/..?*; do
  [ -e \"$path\" ] || continue
  name=$(basename \"$path\")
  [ \"$name\" = .git ] && continue
  for secret in \"$@\"; do [ \"$name\" = \"$secret\" ] && continue 2; done
  [ -e \"/sandbox/$name\" ] || ln -s \"$path\" \"/sandbox/$name\"
done
";

/// Copies each setup folder into the sandbox `name` and links its dotfiles into the home there.
pub fn link_dotfiles(
    openshell: &OpenShell,
    name: &str,
    setups: &[SetupFolder],
) -> Result<(), OpenShellError> {
    for setup in setups {
        // Under a folder named for the kit, so two kits' folders of one name don't meet.
        let staging = std::env::temp_dir()
            .join(format!("playpen-dotfiles-{}", std::process::id()))
            .join(setup.slug());
        let _ = std::fs::remove_dir_all(&staging);
        let copied = (|| -> std::io::Result<()> {
            for file in files(&setup.folder) {
                let target = staging.join(file.strip_prefix(&setup.folder).unwrap_or(&file));
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&file, &target)?;
            }
            Ok(())
        })();
        if let Err(error) = copied {
            return Err(OpenShellError::Failed(
                format!("copy {}'s setup folder", setup.kit),
                error.to_string(),
            ));
        }
        let uploaded = openshell.upload(name, &staging, "/sandbox/.playpen/kits");
        let _ = std::fs::remove_dir_all(staging.parent().unwrap_or(&staging));
        uploaded?;
        let dir = format!("/sandbox/.playpen/kits/{}", setup.slug());
        let mut args = vec![dir.as_str()];
        args.extend(SECRET_FILES);
        openshell.exec_bytes(name, &format!("link {}'s dotfiles", setup.kit), LINK, &args)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SetupFolder, dockerfile, image_name};
    use crate::agent::Agent;

    #[test]
    fn a_kits_install_sh_makes_an_image_named_by_what_went_into_it() {
        let root = std::env::temp_dir().join(format!("kit-setup-{}", std::process::id()));
        let tools = root.join("tools");
        let plain = root.join("plain");
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(tools.join("install.sh"), "apt-get install -y jq\n").unwrap();
        std::fs::write(plain.join(".bashrc"), "alias ll='ls -l'\n").unwrap();
        let setup = |kit: &str, folder: &std::path::Path| SetupFolder {
            kit: kit.to_string(),
            folder: folder.to_path_buf(),
        };
        assert_eq!(
            image_name(Agent::ClaudeCode, &[setup("plain", &plain)]),
            Agent::ClaudeCode.image(),
            "nothing to install, so the agent's own"
        );
        let first = image_name(Agent::ClaudeCode, &[setup("work", &tools)]);
        assert!(first.starts_with("playpen-claude-code-kits:"), "{first}");
        assert_eq!(
            first,
            image_name(Agent::ClaudeCode, &[setup("work", &tools)]),
            "the same again"
        );
        std::fs::write(tools.join("install.sh"), "apt-get install -y jq vim\n").unwrap();
        assert_ne!(
            first,
            image_name(Agent::ClaudeCode, &[setup("work", &tools)]),
            "changed"
        );
        let work = setup("work", &tools);
        insta::assert_snapshot!(dockerfile(Agent::ClaudeCode, &[&work]));
        let _ = std::fs::remove_dir_all(&root);
    }
}
