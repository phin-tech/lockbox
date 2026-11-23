//! What a person has set in the Settings window, kept as a small TOML file. Every setting has a default, a file that's
//! missing or can't be read gives the defaults, and a setting this version doesn't know is left alone.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agent::Agent;
use crate::docker::{Docker, DockerError};
use crate::files::write_atomically;
use crate::image::{ImageSource, Named, Platform, Provided, Registry};
use crate::openshell::GatewayInfo;
use crate::session::ShowIn;

/// Where a gateway gets the images its agents run in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImagesFrom {
    /// Built with this Mac's Docker, for a gateway that shares it.
    Docker,
    /// Built here and pushed to a registry the gateway pulls from.
    Registry,
    /// Already in a registry the gateway pulls from, put there by the person: nothing is built or pushed.
    Provided,
}

/// What a person has set for one gateway, in a `[gateways.<name>]` section of the settings file.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewaySettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<ImagesFrom>,
    /// The registry and folder images are pushed to, or are in: `ghcr.io/phin-tech`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub registry: String,
    /// The CPU the gateway's nodes run, which an image has to be built for. It isn't guessed, since a wrong one gives
    /// an image that won't start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,
}

/// What a person has set for one of Playpen's agents, in an `[agents.<id>]` section of the settings file.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    /// An image of their own to start it from instead of the one Playpen builds, which every gateway pulls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// Why a gateway has no image source to make a sandbox from.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageSourceError {
    #[error(transparent)]
    Docker(#[from] DockerError),
    #[error(
        "“{0}” doesn't say where its images come from. Choose in Gateway ▸ Image Source…, or add a [gateways.{0}] section to the settings file."
    )]
    NotSet(String),
    #[error(
        "“{0}” takes its images from a registry, so it needs one, such as ghcr.io/your-name. Set it in Gateway ▸ Image Source…."
    )]
    NoRegistry(String),
    #[error(
        "“{0}” takes its images from a registry, so it needs a platform, amd64 or arm64, whichever its nodes run. Set it in Gateway ▸ Image Source…."
    )]
    NoPlatform(String),
}

/// The light or dark look: following the system, or fixed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

/// The themes the app starts with, Typecase's, by name.
pub const LIGHT_THEME: &str = "Typecase Light";
pub const DARK_THEME: &str = "Typecase Dark";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub appearance: Appearance,
    /// The theme shown while the app is light, and while it's dark, by name: one of Typecase's, chosen in Settings.
    pub light_theme: String,
    pub dark_theme: String,
    /// How much bigger than the theme's sizes text is drawn: one of [`TEXT_SCALES`].
    pub text_scale: f32,
    /// The shortcut that brings the app forward from any app, as `global-hotkey` reads it: `ctrl+alt+super+Space`.
    pub hotkey: String,
    /// Whether Sparkle offers betas, which come before a release. A beta starts with them on, so it keeps getting
    /// them until they're switched off; a tester moves on to the release that follows, since builds only go up.
    pub beta_updates: bool,
    /// Where sessions are shown unless one is opened somewhere else.
    pub show_in: ShowIn,
    /// Whether herdr-core keeps sessions, when it's installed, so closing a window only detaches from one.
    pub keep_sessions: bool,
    /// What's set for each gateway, by the name OpenShell gives it.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub gateways: BTreeMap<String, GatewaySettings>,
    /// What's set for each of Playpen's agents, by its id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, AgentSettings>,
    /// Where a pasted key is kept besides OpenShell, which the forms start with.
    pub keep_keys_in: KeepKeysIn,
    /// The 1Password vault keys are saved in, by its id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vault: Option<String>,
}

/// Where a pasted key is kept besides OpenShell, so it can go to another gateway or be replaced everywhere without
/// being pasted again.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeepKeysIn {
    #[default]
    Nowhere,
    OnePassword,
    Keychain,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            appearance: Appearance::default(),
            light_theme: LIGHT_THEME.to_string(),
            dark_theme: DARK_THEME.to_string(),
            text_scale: 1.0,
            hotkey: "ctrl+alt+super+Space".to_string(),
            beta_updates: crate::VERSION.contains("-beta"),
            show_in: ShowIn::default(),
            keep_sessions: true,
            gateways: BTreeMap::new(),
            agents: BTreeMap::new(),
            keep_keys_in: KeepKeysIn::default(),
            vault: None,
        }
    }
}

/// The text sizes on offer, as multiples of the theme's own.
pub const TEXT_SCALES: [f32; 7] = [0.85, 1.0, 1.15, 1.3, 1.5, 1.75, 2.0];

/// The scale `steps` along from `current`, staying inside the range. A saved scale that isn't one of the steps moves
/// to the nearest.
pub fn step_text_scale(current: f32, steps: isize) -> f32 {
    let nearest = TEXT_SCALES
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (*a - current).abs().total_cmp(&(*b - current).abs()))
        .map_or(1, |(index, _)| index);
    let index = (nearest as isize + steps).clamp(0, TEXT_SCALES.len() as isize - 1);
    TEXT_SCALES[index as usize]
}

impl Settings {
    /// The image of the person's own that `agent` starts from, if they gave one.
    pub fn agent_image(&self, agent: Agent) -> Option<String> {
        self.agents
            .get(agent.id())
            .and_then(|set| set.image.as_deref())
            .map(str::trim)
            .filter(|image| !image.is_empty())
            .map(str::to_string)
    }

    /// Starts `agent` from `image`, or from Playpen's own again with none.
    pub fn set_agent_image(&mut self, agent: Agent, image: Option<String>) {
        let image = image
            .map(|image| image.trim().to_string())
            .filter(|image| !image.is_empty());
        match image {
            Some(image) => {
                self.agents.entry(agent.id().to_string()).or_default().image = Some(image);
            }
            None => {
                self.agents.remove(agent.id());
            }
        }
    }

    /// Where `gateway` gets a sandbox's image, when the sandbox may name its own: then a gateway that hasn't said
    /// where its images come from still makes it, since nothing is built or pushed.
    pub fn image_source_for(
        &self,
        gateway: &GatewayInfo,
        names_its_image: bool,
        docker: impl FnOnce() -> Result<Docker, DockerError>,
    ) -> Result<Box<dyn ImageSource>, ImageSourceError> {
        match self.image_source(gateway, docker) {
            Err(
                ImageSourceError::NotSet(_)
                | ImageSourceError::NoRegistry(_)
                | ImageSourceError::NoPlatform(_),
            ) if names_its_image => Ok(Box::new(Named::new(Platform::host()))),
            source => source,
        }
    }

    /// Where `gateway` gets its agents' images. One on this Mac shares its Docker, so it needs nothing said; any other
    /// has to say, since there's no telling what it can pull from. `docker` finds Docker, which only some sources use.
    pub fn image_source(
        &self,
        gateway: &GatewayInfo,
        docker: impl FnOnce() -> Result<Docker, DockerError>,
    ) -> Result<Box<dyn ImageSource>, ImageSourceError> {
        let set = self.gateways.get(&gateway.name);
        let name = &gateway.name;
        let from = match set.and_then(|set| set.images) {
            Some(from) => from,
            None if gateway.is_local() => ImagesFrom::Docker,
            None => return Err(ImageSourceError::NotSet(name.clone())),
        };
        if from == ImagesFrom::Docker {
            return Ok(Box::new(docker()?));
        }
        let set = set.cloned().unwrap_or_default();
        if set.registry.trim().is_empty() {
            return Err(ImageSourceError::NoRegistry(name.clone()));
        }
        let platform = set
            .platform
            .ok_or_else(|| ImageSourceError::NoPlatform(name.clone()))?;
        Ok(match from {
            ImagesFrom::Provided => Box::new(Provided::new(set.registry, platform)),
            _ => Box::new(Registry::new(docker()?, set.registry, platform)),
        })
    }

    /// The settings in `path`, or the defaults if there's no file there or it doesn't parse.
    pub fn load(path: &Path) -> Settings {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the settings to `path`, making its folder if need be. Settings the file has that this version doesn't
    /// know are kept, so going back and forth between versions loses nothing.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut table: toml::Table = fs::read_to_string(path)
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or_default();
        let ours = toml::Table::try_from(self).map_err(io::Error::other)?;
        table.extend(ours);
        // Left out when there are none, so what the file had would stay: the last gateway's settings can be removed.
        if self.gateways.is_empty() {
            table.remove("gateways");
        }
        if self.agents.is_empty() {
            table.remove("agents");
        }
        let text = toml::to_string(&table).map_err(io::Error::other)?;
        write_atomically(path, text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentSettings, Appearance, GatewaySettings, ImageSourceError, ImagesFrom, KeepKeysIn,
        Settings, TEXT_SCALES, step_text_scale,
    };
    use crate::agent::Agent;
    use crate::docker::{Docker, DockerError};
    use crate::image::Platform;
    use crate::openshell::{GatewayInfo, parse_gateways};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    fn gateway(name: &str, kind: &str, is_remote: bool) -> GatewayInfo {
        parse_gateways(&format!(
            r#"[{{"name": "{name}", "type": "{kind}", "is_remote": {is_remote}}}]"#
        ))
        .unwrap()
        .remove(0)
    }

    fn with(name: &str, set: GatewaySettings) -> Settings {
        Settings {
            gateways: BTreeMap::from([(name.to_string(), set)]),
            ..Settings::default()
        }
    }

    fn no_docker() -> Result<Docker, DockerError> {
        Err(DockerError::NotInstalled)
    }

    fn some_docker() -> Result<Docker, DockerError> {
        Ok(Docker::at("/usr/local/bin/docker"))
    }

    #[test]
    fn a_gateway_on_this_mac_gets_its_images_from_docker_without_being_told() {
        let local = gateway("openshell", "local", false);
        let source = Settings::default()
            .image_source(&local, some_docker)
            .unwrap();
        assert_eq!(source.platform(), Platform::host());
        let without = Settings::default().image_source(&local, no_docker);
        assert!(matches!(
            without,
            Err(ImageSourceError::Docker(DockerError::NotInstalled))
        ));
    }

    #[test]
    fn any_other_gateway_has_to_say_where_its_images_come_from() {
        let cluster = gateway("k8s-home", "remote", true);
        let error = Settings::default()
            .image_source(&cluster, some_docker)
            .err()
            .unwrap();
        assert_eq!(error, ImageSourceError::NotSet("k8s-home".to_string()));
        let said = error.to_string();
        assert!(said.contains("Image Source…") && said.contains("[gateways.k8s-home]"));
    }

    #[test]
    fn a_registry_needs_a_prefix_and_the_platform_its_nodes_run() {
        let cluster = gateway("k8s-home", "remote", true);
        let registry = |registry: &str, platform: Option<Platform>| {
            with(
                "k8s-home",
                GatewaySettings {
                    images: Some(ImagesFrom::Registry),
                    registry: registry.to_string(),
                    platform,
                },
            )
            .image_source(&cluster, some_docker)
            .err()
        };
        assert_eq!(
            registry("", Some(Platform::Amd64)),
            Some(ImageSourceError::NoRegistry("k8s-home".to_string()))
        );
        assert_eq!(
            registry("ghcr.io/phin-tech", None),
            Some(ImageSourceError::NoPlatform("k8s-home".to_string()))
        );
        assert_eq!(registry("ghcr.io/phin-tech", Some(Platform::Arm64)), None);
    }

    #[test]
    fn an_image_the_person_provides_needs_no_docker_and_is_named_under_the_registry() {
        let cluster = gateway("k8s-home", "remote", true);
        let settings = with(
            "k8s-home",
            GatewaySettings {
                images: Some(ImagesFrom::Provided),
                registry: "ghcr.io/phin-tech".to_string(),
                platform: Some(Platform::Amd64),
            },
        );
        let source = settings.image_source(&cluster, no_docker).unwrap();
        assert_eq!(source.platform(), Platform::Amd64);
        let image = Agent::Codex.build();
        let reference = source.ready(&image, &|_| {}).unwrap();
        assert_eq!(reference, format!("ghcr.io/phin-tech/{}", image.tag));
    }

    #[test]
    fn the_last_gateways_settings_can_be_removed_and_other_settings_are_kept() {
        let path = scratch("last-gateway");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "from_the_future = 3\n[gateways.k8s-home]\nimages = \"docker\"\n",
        )
        .unwrap();
        let mut settings = Settings::load(&path);
        assert!(settings.gateways.contains_key("k8s-home"));
        settings.gateways.clear();
        settings.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("gateways"), "{text}");
        assert!(text.contains("from_the_future = 3"), "{text}");
        assert!(Settings::load(&path).gateways.is_empty());
    }

    #[test]
    fn a_gateways_settings_read_plainly_from_the_file() {
        let text = "[gateways.k8s-home]\nimages = \"registry\"\nregistry = \"ghcr.io/phin-tech\"\nplatform = \"amd64\"\n";
        let settings: Settings = toml::from_str(text).unwrap();
        let set = &settings.gateways["k8s-home"];
        assert_eq!(set.images, Some(ImagesFrom::Registry));
        assert_eq!(set.platform, Some(Platform::Amd64));
        assert_eq!(settings.appearance, Appearance::System);
    }

    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        folder.join("settings.toml")
    }

    #[test]
    fn settings_come_back_as_they_were_saved() {
        let path = scratch("round-trip");
        let settings = Settings {
            appearance: Appearance::Dark,
            light_theme: "Gruvppuccin Light".to_string(),
            dark_theme: "Nightbrew Cove".to_string(),
            text_scale: 1.3,
            hotkey: "ctrl+alt+super+KeyN".to_string(),
            beta_updates: true,
            show_in: crate::session::ShowIn::Ghostty,
            keep_sessions: false,
            gateways: BTreeMap::from([(
                "k8s-home".to_string(),
                GatewaySettings {
                    images: Some(ImagesFrom::Registry),
                    registry: "ghcr.io/phin-tech".to_string(),
                    platform: Some(Platform::Amd64),
                },
            )]),
            agents: BTreeMap::from([(
                "pi".to_string(),
                AgentSettings {
                    image: Some("ghcr.io/me/pi-tools:1".to_string()),
                },
            )]),
            keep_keys_in: KeepKeysIn::OnePassword,
            vault: Some("vault-id".to_string()),
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
    }

    #[test]
    fn an_agent_starts_from_the_persons_image_until_it_is_cleared() {
        let mut settings = Settings::default();
        assert_eq!(settings.agent_image(Agent::Pi), None);
        settings.set_agent_image(Agent::Pi, Some(" ghcr.io/me/pi-tools:1 ".to_string()));
        assert_eq!(
            settings.agent_image(Agent::Pi).as_deref(),
            Some("ghcr.io/me/pi-tools:1")
        );
        settings.set_agent_image(Agent::Pi, Some(String::new()));
        assert_eq!(settings.agent_image(Agent::Pi), None);
        assert!(settings.agents.is_empty(), "nothing left behind");
    }

    #[test]
    fn a_sandbox_naming_its_image_is_made_on_a_gateway_that_hasnt_said_where_images_come_from() {
        let cluster = gateway("k8s", "remote", true);
        let settings = Settings::default();
        assert!(matches!(
            settings.image_source_for(&cluster, false, no_docker),
            Err(ImageSourceError::NotSet(_))
        ));
        let named = settings
            .image_source_for(&cluster, true, no_docker)
            .unwrap();
        assert_eq!(named.platform(), Platform::host());
    }

    #[test]
    fn a_settings_file_reads_plainly() {
        let settings = Settings {
            appearance: Appearance::System,
            light_theme: "Typecase Light".to_string(),
            dark_theme: "Typecase Dark".to_string(),
            text_scale: 1.0,
            hotkey: "ctrl+alt+super+Space".to_string(),
            beta_updates: false,
            show_in: crate::session::ShowIn::Automatic,
            keep_sessions: true,
            gateways: BTreeMap::new(),
            agents: BTreeMap::new(),
            keep_keys_in: KeepKeysIn::Nowhere,
            vault: None,
        };
        let text = toml::to_string(&settings).unwrap();
        insta::assert_snapshot!(text, @r#"
        appearance = "system"
        light_theme = "Typecase Light"
        dark_theme = "Typecase Dark"
        text_scale = 1.0
        hotkey = "ctrl+alt+super+Space"
        beta_updates = false
        show_in = "automatic"
        keep_sessions = true
        keep_keys_in = "nowhere"
        "#);
    }

    #[test]
    fn a_missing_or_broken_file_gives_the_defaults() {
        let path = scratch("broken");
        assert_eq!(Settings::load(&path), Settings::default());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "appearance = [").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn saving_keeps_settings_from_another_version() {
        let path = scratch("unknown");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "appearance = \"light\"\nfrom_the_future = 3\n").unwrap();
        let mut settings = Settings::load(&path);
        assert_eq!(settings.appearance, Appearance::Light);
        settings.appearance = Appearance::System;
        settings.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("from_the_future = 3"), "{text}");
        assert!(text.contains("appearance = \"system\""), "{text}");
    }

    #[test]
    fn text_size_steps_stop_at_both_ends() {
        let last = TEXT_SCALES[TEXT_SCALES.len() - 1];
        assert_eq!(step_text_scale(1.0, 1), 1.15);
        assert_eq!(step_text_scale(1.0, -5), TEXT_SCALES[0]);
        assert_eq!(step_text_scale(last, 1), last);
    }

    #[test]
    fn an_odd_saved_text_size_steps_from_the_nearest() {
        assert_eq!(step_text_scale(1.2, 0), 1.15);
        assert_eq!(step_text_scale(9.0, -1), 1.75);
    }
}
