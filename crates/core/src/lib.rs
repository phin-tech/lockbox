//! What Lockbox does, free of UI, so tests reach it without a window. Replace [`Tally`] with the app's own model.

pub mod crash;
mod files;
mod links;
mod locale;
mod settings;
mod state;

pub use files::write_atomically;
pub use links::{link_target, path_from_file_url};
pub use locale::best_locale;
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

/// Why a file isn't a tally.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TallyError {
    #[error("it isn't a tally file: {0}")]
    NotToml(String),
    #[error("there's no count in it")]
    NoCount,
    #[error("{0} isn't a count")]
    BadCount(i64),
}

/// A count that goes up and can start over.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tally {
    count: u32,
}

impl Tally {
    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn add(&mut self) {
        self.count = self.count.saturating_add(1);
    }

    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// The tally as a file holds it: `count = 3`.
    pub fn to_text(&self) -> String {
        format!("count = {}\n", self.count)
    }

    /// A tally read back from [`Tally::to_text`], or why it can't be.
    pub fn from_text(text: &str) -> Result<Tally, TallyError> {
        let table: toml::Table = text
            .parse()
            .map_err(|error: toml::de::Error| TallyError::NotToml(error.message().to_string()))?;
        let count = table
            .get("count")
            .and_then(toml::Value::as_integer)
            .ok_or(TallyError::NoCount)?;
        let count = u32::try_from(count).map_err(|_| TallyError::BadCount(count))?;
        Ok(Tally { count })
    }
}

#[cfg(test)]
mod tests {
    use super::{BUILD, Tally, TallyError, VERSION, version};

    #[test]
    fn a_tally_counts_up_and_starts_over() {
        let mut tally = Tally::default();
        tally.add();
        tally.add();
        assert_eq!(tally.count(), 2);
        tally.clear();
        assert_eq!(tally.count(), 0);
    }

    #[test]
    fn a_full_tally_stays_full() {
        let mut tally = Tally { count: u32::MAX };
        tally.add();
        assert_eq!(tally.count(), u32::MAX);
    }

    #[test]
    fn a_tally_comes_back_from_its_file() {
        let mut tally = Tally::default();
        tally.add();
        tally.add();
        insta::assert_snapshot!(tally.to_text(), @"count = 2");
        assert_eq!(Tally::from_text(&tally.to_text()), Ok(tally));
    }

    #[test]
    fn a_file_that_isnt_a_tally_says_why() {
        assert_eq!(
            Tally::from_text("count = -1"),
            Err(TallyError::BadCount(-1))
        );
        assert_eq!(Tally::from_text("nothing = 1"), Err(TallyError::NoCount));
        let broken = Tally::from_text("count = ").unwrap_err();
        assert!(matches!(broken, TallyError::NotToml(_)), "{broken}");
    }

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
