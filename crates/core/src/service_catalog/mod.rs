//! A bundled, shareable JSON catalog of common developer services Playpen can add to OpenShell (including Patchbay's
//! local MCP gateway on this Mac, GitHub, Cloudflare, Google Cloud, AWS Bedrock, Statsig, Sentry, Linear and more).
//!
//! Built-in presets compile into the binary (`services.json`), and Playpen merges any extra JSON files placed in
//! `~/Library/Application Support/tech.phin.playpen/services.json` or `services/*.json` so teams can share their own
//! service definitions.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::new_service::{Access, NewService, ServiceAuth, is_variable, patchbay_port};
use crate::provider::Endpoint;

/// The catalog JSON schema version this build reads and writes.
pub const VERSION: u32 = 1;

/// The built-in curated services JSON, compiled into the binary so the app needs no extra files in its bundle.
pub const BUILTIN_JSON: &str = include_str!("services.json");

/// A shareable service catalog JSON file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceCatalogFile {
    pub version: u32,
    #[serde(default)]
    pub services: Vec<ServicePreset>,
}

/// The loaded service catalog: built-in presets merged with any custom JSON files in Application Support.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceCatalog {
    pub services: Vec<ServicePreset>,
}

/// How presets are grouped in the Add Service picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceCategory {
    Gateway,
    Cloud,
    Code,
    Observability,
    Databases,
    Productivity,
    Packages,
}

impl ServiceCategory {
    pub const ALL: [ServiceCategory; 7] = [
        ServiceCategory::Gateway,
        ServiceCategory::Cloud,
        ServiceCategory::Code,
        ServiceCategory::Observability,
        ServiceCategory::Databases,
        ServiceCategory::Productivity,
        ServiceCategory::Packages,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ServiceCategory::Gateway => "Patchbay",
            ServiceCategory::Cloud => "Cloud",
            ServiceCategory::Code => "Code",
            ServiceCategory::Observability => "Observability",
            ServiceCategory::Databases => "Databases",
            ServiceCategory::Productivity => "Productivity",
            ServiceCategory::Packages => "Packages",
        }
    }
}

/// How a preset's key is sent on requests, as written in `services.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresetAuth {
    #[default]
    Bearer,
    Header,
}

/// Default access level for a preset, as written in `services.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresetAccess {
    #[default]
    ReadOnly,
    ReadWrite,
}

impl From<PresetAccess> for Access {
    fn from(access: PresetAccess) -> Access {
        match access {
            PresetAccess::ReadOnly => Access::ReadOnly,
            PresetAccess::ReadWrite => Access::ReadWrite,
        }
    }
}

/// One developer service preset in `services.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServicePreset {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub category: ServiceCategory,
    /// Primary host (or `host:port`) the key is sent to.
    pub host: String,
    /// Optional extra hosts that receive the same key (e.g. multiple Google Cloud REST APIs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_hosts: Vec<String>,
    /// Environment variable name for the stand-in inside the sandbox.
    pub variable: String,
    #[serde(default)]
    pub auth: PresetAuth,
    /// Required when `auth` is `"header"` (e.g. `STATSIG-API-KEY`, `DD-API-KEY`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default)]
    pub access: PresetAccess,
    /// Where to create or copy a key for this service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Extra setup hint shown in the Add Service form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Optional pre-wired macOS Keychain service name (e.g. `"Patchbay"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keychain_service: Option<String>,
    /// Optional pre-wired macOS Keychain account name (e.g. `"gateway/token"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keychain_account: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Which custom JSON file added or replaced this preset; none for built-in entries.
    #[serde(skip)]
    pub source: Option<String>,
}

/// Why a service catalog JSON file couldn't be read or saved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceCatalogError {
    #[error("That isn't valid service catalog JSON: {0}")]
    Json(String),
    #[error("This catalog is version {0}, which this Playpen doesn't understand yet")]
    UnsupportedVersion(u32),
    #[error("The catalog has no services")]
    Empty,
    #[error("“{0}” isn't a valid service ID")]
    BadId(String),
    #[error("“{0}” isn't a valid environment variable name")]
    BadVariable(String),
    #[error("“{0}” isn't a valid host or host:port")]
    BadHost(String),
    #[error("Service “{0}” uses header auth without naming its header")]
    MissingHeader(String),
    #[error("Couldn't save the service catalog: {0}")]
    Io(String),
    #[error("Couldn't read {0}")]
    Read(String),
}

impl ServiceCatalogFile {
    pub fn from_json(text: &str) -> Result<ServiceCatalogFile, ServiceCatalogError> {
        let file: ServiceCatalogFile = serde_json::from_str(text)
            .map_err(|error| ServiceCatalogError::Json(error.to_string()))?;
        file.validate()?;
        Ok(file)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a service catalog is always JSON")
    }

    pub fn validate(&self) -> Result<(), ServiceCatalogError> {
        if self.version != VERSION {
            return Err(ServiceCatalogError::UnsupportedVersion(self.version));
        }
        if self.services.is_empty() {
            return Err(ServiceCatalogError::Empty);
        }
        for service in &self.services {
            if !crate::is_sandbox_name(&service.id) {
                return Err(ServiceCatalogError::BadId(service.id.clone()));
            }
            if !is_variable(&service.variable) {
                return Err(ServiceCatalogError::BadVariable(service.variable.clone()));
            }
            if Endpoint::from_address(&service.host).is_err() {
                return Err(ServiceCatalogError::BadHost(service.host.clone()));
            }
            for extra in &service.extra_hosts {
                if Endpoint::from_address(extra).is_err() {
                    return Err(ServiceCatalogError::BadHost(extra.clone()));
                }
            }
            if service.auth == PresetAuth::Header
                && service
                    .header
                    .as_deref()
                    .is_none_or(|header| header.trim().is_empty())
            {
                return Err(ServiceCatalogError::MissingHeader(service.id.clone()));
            }
        }
        Ok(())
    }
}

impl ServicePreset {
    /// The primary host, substituting Patchbay's live gateway port when the preset is `patchbay`.
    pub fn resolved_host(&self) -> String {
        if self.id == "patchbay" && self.host.starts_with("host.openshell.internal:") {
            format!("host.openshell.internal:{}", patchbay_port())
        } else {
            self.host.clone()
        }
    }

    /// How the key is sent on requests.
    pub fn service_auth(&self) -> ServiceAuth {
        match (self.auth, &self.header) {
            (PresetAuth::Header, Some(header)) if !header.trim().is_empty() => {
                ServiceAuth::Header(header.trim().to_string())
            }
            _ => ServiceAuth::Bearer,
        }
    }

    /// Extra endpoints parsed from `extra_hosts`.
    pub fn extra_endpoints(&self) -> Vec<Endpoint> {
        self.extra_hosts
            .iter()
            .filter_map(|host| Endpoint::from_address(host).ok())
            .collect()
    }

    /// Builds a [`NewService`] from this preset with its default name, host, variable, auth and access.
    pub fn to_new_service(&self) -> Result<NewService, crate::provider::ProviderError> {
        Ok(NewService::new(
            &self.id,
            &self.resolved_host(),
            &self.variable,
            self.access.into(),
        )?
        .with_auth(self.service_auth())
        .with_extra_endpoints(self.extra_endpoints()))
    }

    fn matches_term(&self, term: &str) -> bool {
        self.id.to_lowercase().contains(term)
            || self.name.to_lowercase().contains(term)
            || self.summary.to_lowercase().contains(term)
            || self.host.to_lowercase().contains(term)
            || self.variable.to_lowercase().contains(term)
            || self.category.label().to_lowercase().contains(term)
            || self
                .extra_hosts
                .iter()
                .any(|host| host.to_lowercase().contains(term))
            || self
                .tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(term))
            || self
                .source
                .as_deref()
                .is_some_and(|src| src.to_lowercase().contains(term))
    }
}

impl ServiceCatalog {
    /// The curated catalog compiled into Playpen.
    pub fn builtin() -> ServiceCatalog {
        let file =
            ServiceCatalogFile::from_json(BUILTIN_JSON).expect("built-in services.json is valid");
        ServiceCatalog {
            services: file.services,
        }
    }

    /// Built-in presets merged with `<dir>/services.json` and `<dir>/services/*.json` when `dir` is given.
    pub fn load(dir: Option<&Path>) -> (ServiceCatalog, Vec<String>) {
        let mut catalog = ServiceCatalog::builtin();
        let mut errors = Vec::new();
        let Some(dir) = dir else {
            return (catalog, errors);
        };
        for path in catalog_files(dir) {
            let label = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("services.json")
                .to_string();
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) => {
                    errors.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            match ServiceCatalogFile::from_json(&text) {
                Ok(file) => catalog.merge(file.services, &label),
                Err(error) => errors.push(format!("{}: {error}", path.display())),
            }
        }
        (catalog, errors)
    }

    /// Merges `extra` presets from `source`, replacing any preset of the same ID.
    pub fn merge(&mut self, extra: Vec<ServicePreset>, source: &str) {
        for mut service in extra {
            service.source = Some(source.to_string());
            if let Some(existing) = self.services.iter_mut().find(|item| item.id == service.id) {
                *existing = service;
            } else {
                self.services.push(service);
            }
        }
    }

    /// Serializes the whole catalog (or only presets matching `ids`, if non-empty) as pretty shareable JSON.
    pub fn export_json(&self, ids: &[String]) -> String {
        let services = if ids.is_empty() {
            self.services.clone()
        } else {
            self.services
                .iter()
                .filter(|service| ids.iter().any(|id| id == &service.id))
                .cloned()
                .collect()
        };
        ServiceCatalogFile {
            version: VERSION,
            services,
        }
        .to_json()
    }

    /// The file name a shared catalog is kept under in `<dir>/services/`: its own, lowercased, with anything but
    /// letters, digits, `-` and `_` made a `-`.
    pub fn stored_name(from: &Path) -> String {
        let stem = from
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("shared");
        let clean: String = stem
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let clean = clean.trim_matches('-');
        format!("{}.json", if clean.is_empty() { "shared" } else { clean })
    }

    /// Validates a shared catalog JSON file and copies it into `<dir>/services/<filename>`.
    pub fn import_file(dir: &Path, from: &Path) -> Result<(String, usize), ServiceCatalogError> {
        let text = std::fs::read_to_string(from)
            .map_err(|error| ServiceCatalogError::Read(format!("{}: {error}", from.display())))?;
        let file = ServiceCatalogFile::from_json(&text)?;
        let filename = ServiceCatalog::stored_name(from);
        let folder = dir.join("services");
        std::fs::create_dir_all(&folder)
            .map_err(|error| ServiceCatalogError::Io(error.to_string()))?;
        crate::write_atomically(&folder.join(&filename), text.as_bytes())
            .map_err(|error| ServiceCatalogError::Io(error.to_string()))?;
        Ok((filename, file.services.len()))
    }

    pub fn filter(&self, query: &str, category: Option<ServiceCategory>) -> Vec<&ServicePreset> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|term| term.to_lowercase())
            .collect();
        self.services
            .iter()
            .filter(|service| category.is_none_or(|wanted| service.category == wanted))
            .filter(|service| terms.iter().all(|term| service.matches_term(term)))
            .collect()
    }

    pub fn find(&self, id: &str) -> Option<&ServicePreset> {
        self.services.iter().find(|service| service.id == id)
    }
}

/// Every custom service catalog JSON under `dir`: `services.json`, then `services/*.json` sorted by name.
fn catalog_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let single = dir.join("services.json");
    if single.is_file() {
        files.push(single);
    }
    let mut extra: Vec<PathBuf> = std::fs::read_dir(dir.join("services"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        })
        .collect();
    extra.sort();
    files.extend(extra);
    files
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{ServiceCatalog, ServiceCatalogFile, ServiceCategory};

    fn scratch(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "playpen-service-catalog-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn every_builtin_service_preset_is_valid_and_builds_an_openshell_profile() {
        let catalog = ServiceCatalog::builtin();
        assert!(catalog.services.len() >= 15);
        assert_eq!(catalog.services[0].id, "patchbay");
        let mut ids = BTreeSet::new();
        for preset in &catalog.services {
            assert!(ids.insert(preset.id.clone()), "duplicate id: {}", preset.id);
            let new = preset.to_new_service().unwrap();
            let yaml = new.profile();
            assert!(yaml.contains(&format!("id: playpen-service-{}", preset.id)));
            assert!(yaml.contains(&format!("env_vars: [{}]", preset.variable)));
            for extra in &preset.extra_hosts {
                assert!(
                    yaml.contains(extra),
                    "{} missing extra host {extra}",
                    preset.id
                );
            }
        }
    }

    #[test]
    fn statsig_and_gcp_presets_use_their_exact_headers_and_endpoints() {
        let catalog = ServiceCatalog::builtin();
        let statsig = catalog.find("statsig").unwrap().to_new_service().unwrap();
        let yaml = statsig.profile();
        assert!(yaml.contains("auth_style: header"), "{yaml}");
        assert!(yaml.contains("header_name: STATSIG-API-KEY"), "{yaml}");
        assert!(yaml.contains("host: statsigapi.net"), "{yaml}");

        let gcp = catalog.find("gcp").unwrap().to_new_service().unwrap();
        let yaml = gcp.profile();
        assert!(yaml.contains("host: bigquery.googleapis.com"), "{yaml}");
        assert!(yaml.contains("host: run.googleapis.com"), "{yaml}");
        assert!(yaml.contains("host: logging.googleapis.com"), "{yaml}");
    }

    #[test]
    fn custom_services_json_merges_overrides_and_exports_cleanly() {
        let root = scratch("merge");
        let shared = r#"{
          "version": 1,
          "services": [
            {
              "id": "acme-api",
              "name": "Acme API",
              "summary": "Internal staging API",
              "category": "cloud",
              "host": "api.acme.internal",
              "variable": "ACME_API_KEY",
              "auth": "header",
              "header": "X-Acme-Key",
              "access": "read-only"
            }
          ]
        }"#;
        let source = root.join("Team Services.json");
        std::fs::write(&source, shared).unwrap();
        let (stored, count) = ServiceCatalog::import_file(&root, &source).unwrap();
        assert_eq!(stored, "team-services.json");
        assert_eq!(count, 1);

        let (catalog, errors) = ServiceCatalog::load(Some(&root));
        assert!(errors.is_empty(), "{errors:?}");
        let acme = catalog.find("acme-api").unwrap();
        assert_eq!(acme.source.as_deref(), Some("team-services.json"));
        assert_eq!(
            catalog.filter("acme", Some(ServiceCategory::Cloud)).len(),
            1
        );

        let exported = catalog.export_json(&["acme-api".to_string()]);
        let reparsed = ServiceCatalogFile::from_json(&exported).unwrap();
        assert_eq!(reparsed.services.len(), 1);
        assert_eq!(reparsed.services[0].id, "acme-api");

        std::fs::remove_dir_all(&root).unwrap();
    }
}
