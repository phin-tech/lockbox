//! What Playpen does, free of UI, so tests reach it without a window: OpenShell's sandboxes, and the agents Playpen
//! makes them for.

mod added;
mod agent;
mod bring_back;
mod chatgpt;
pub mod crash;
mod docker;
mod dotenv;
mod files;
mod herdr;
mod kit_setup;
mod kits;
mod links;
mod locale;
mod new_service;
mod openshell;
mod provider;
mod secrets;
pub mod session;
mod settings;
mod state;

pub use added::{AddError, AddedFolders};
pub use agent::{Agent, MakeError, Plan, Step, is_sandbox_name, open_command, suggested_name};
pub use bring_back::{
    BringBackError, Brought, Repo, branch as bring_back_branch, bring_back, seed,
};
pub use chatgpt::{Account, ChatGptError, DeviceCode, Login, Tokens, find_codex};
pub use docker::{Docker, DockerError};
pub use dotenv::{
    EnvLine, Guess, guess as guess_env, looks_secret, parse as parse_env,
    suggested_name as env_provider_name,
};
pub use files::write_atomically;
pub use herdr::{AgentState, HerdrApp, HerdrCore, HerdrError};
pub use kit_setup::SetupFolder;
pub use kits::{Kit, Kits, Variable, VariableValue};
pub use links::{link_target, path_from_file_url};
pub use locale::best_locale;
pub use new_service::{Access, KeySource, NewService, find_op};
pub use openshell::{
    Gateway, Key, OpenShell, OpenShellError, Phase, Profile, ProfileEndpoint, Provider, Sandbox,
    Service,
};
pub use provider::{Endpoint, Kind, NewProvider, ProviderError};
pub use secrets::Secret;
pub use session::{Installed, Keep, Opened, Place, SessionError, ShowIn, What};
pub use settings::{Appearance, Settings, TEXT_SCALES, step_text_scale};
pub use state::{Frame, Screen, State};

/// The app's version: the first `version` in the workspace's Cargo.toml.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The build number beside it, under `[workspace.metadata.release]`. `just bump` raises both.
pub const BUILD: &str = env!("BUILD");

/// The version as the About box shows it, with the build after it: `0.1.0 (1)`.
pub fn version() -> String {
    format!("{VERSION} ({BUILD})")
}

#[cfg(test)]
mod tests {
    use super::{BUILD, VERSION, version};

    #[test]
    fn the_version_and_build_are_the_ones_in_cargo_toml() {
        let manifest = include_str!("../../../Cargo.toml");
        let table: toml::Table = manifest.parse().unwrap();
        let workspace = &table["workspace"];
        assert_eq!(workspace["package"]["version"].as_str(), Some(VERSION));
        assert_eq!(
            workspace["metadata"]["release"]["build"].as_str(),
            Some(BUILD)
        );
        assert_eq!(version(), format!("{VERSION} ({BUILD})"));
    }
}
