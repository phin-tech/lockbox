//! Registering a gateway with OpenShell, signing in to it and forgetting it: `openshell gateway add`, `login` and
//! `remove`. OpenShell keeps the list; Playpen has none of its own.

use std::path::{Path, PathBuf};

use super::{GatewayInfo, OpenShell, OpenShellError};

/// Why an address can't be registered as a gateway.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NewGatewayError {
    #[error("Give the gateway's address.")]
    NoAddress,
    #[error(
        "“{0}” isn't an address Playpen can register: it starts with https://, ssh:// or http://."
    )]
    Scheme(String),
    #[error("“{0}” can't name a gateway: use letters, digits, dots, dashes and underscores.")]
    Name(String),
    #[error(
        "Give the address of the OIDC issuer that signs people in, such as https://login.example.com/realms/openshell."
    )]
    NoIssuer,
    #[error("“{0}” isn't an OIDC issuer's address: it starts with https://.")]
    Issuer(String),
    #[error("A gateway that signs in with OIDC is reached at an https:// address.")]
    OidcNeedsHttps,
}

/// How an OIDC gateway has people sign in: the issuer, and what to ask it for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Oidc {
    pub issuer: String,
    /// The client the sign-in is for; OpenShell uses `openshell-cli` when it's left empty.
    pub client_id: String,
    /// What the gateway's API is called to the issuer, when it isn't the client.
    pub audience: String,
    /// What to ask for besides sign-in, separated by spaces.
    pub scopes: String,
}

/// How a gateway is reached, which its address says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// `https://`: OpenShell opens a browser to sign in.
    Browser,
    /// `ssh://user@host:port`: a gateway on another machine, over SSH, whose certificates have to be in OpenShell's
    /// folder for it already.
    Ssh,
    /// `http://`: no encryption and no sign-in, for development.
    PlainHttp,
}

/// A gateway to register: where it is, and the name OpenShell is to give it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewGateway {
    endpoint: String,
    name: Option<String>,
    oidc: Option<Oidc>,
}

impl NewGateway {
    /// `endpoint` as typed, and `name`, which may be empty for OpenShell to make one from the address.
    pub fn new(endpoint: &str, name: &str) -> Result<NewGateway, NewGatewayError> {
        let endpoint = endpoint.trim();
        if endpoint.is_empty() {
            return Err(NewGatewayError::NoAddress);
        }
        if Reach::of(endpoint).is_none() {
            return Err(NewGatewayError::Scheme(endpoint.to_string()));
        }
        let name = name.trim();
        if !name.is_empty() && !is_gateway_name(name) {
            return Err(NewGatewayError::Name(name.to_string()));
        }
        Ok(NewGateway {
            endpoint: endpoint.to_string(),
            name: (!name.is_empty()).then(|| name.to_string()),
            oidc: None,
        })
    }

    /// The same gateway, signed in to through `oidc`'s issuer.
    pub fn with_oidc(mut self, oidc: Oidc) -> Result<NewGateway, NewGatewayError> {
        if self.reach() != Reach::Browser {
            return Err(NewGatewayError::OidcNeedsHttps);
        }
        let issuer = oidc.issuer.trim();
        if issuer.is_empty() {
            return Err(NewGatewayError::NoIssuer);
        }
        if !issuer.to_ascii_lowercase().starts_with("https://") {
            return Err(NewGatewayError::Issuer(issuer.to_string()));
        }
        self.oidc = Some(Oidc {
            issuer: issuer.to_string(),
            client_id: oidc.client_id.trim().to_string(),
            audience: oidc.audience.trim().to_string(),
            scopes: oidc.scopes.split_whitespace().collect::<Vec<_>>().join(" "),
        });
        Ok(self)
    }

    pub fn reach(&self) -> Reach {
        Reach::of(&self.endpoint).unwrap_or(Reach::Browser)
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// What follows `openshell`.
    fn args(&self) -> Vec<String> {
        let mut args = vec![
            "gateway".to_string(),
            "add".to_string(),
            self.endpoint.clone(),
        ];
        if let Some(name) = &self.name {
            args.extend(["--name".to_string(), name.clone()]);
        }
        if let Some(oidc) = &self.oidc {
            args.extend(["--oidc-issuer".to_string(), oidc.issuer.clone()]);
            for (flag, value) in [
                ("--oidc-client-id", &oidc.client_id),
                ("--oidc-audience", &oidc.audience),
                ("--oidc-scopes", &oidc.scopes),
            ] {
                if !value.is_empty() {
                    args.extend([flag.to_string(), value.clone()]);
                }
            }
        }
        args
    }
}

impl Reach {
    fn of(endpoint: &str) -> Option<Reach> {
        let scheme = endpoint.split_once("://")?.0.to_ascii_lowercase();
        match scheme.as_str() {
            "https" => Some(Reach::Browser),
            "ssh" => Some(Reach::Ssh),
            "http" => Some(Reach::PlainHttp),
            _ => None,
        }
    }
}

/// Whether `name` is one Playpen will give a gateway. It's a folder's name in OpenShell's configuration, which Remove
/// goes to, so nothing that could leave that folder is allowed.
fn is_gateway_name(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// What was left of a gateway after OpenShell forgot it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Forgotten {
    /// Nothing: its folder was deleted, or never there.
    Clean,
    /// Its folder, with the certificates in it, which Playpen doesn't delete: they're a local gateway's own, and the
    /// gateway is still running.
    Left(PathBuf),
}

/// OpenShell's folder for a gateway, under `config`: where it keeps its certificates and its last sandbox.
fn gateway_folder(config: &Path, name: &str) -> Option<PathBuf> {
    is_gateway_name(name).then(|| config.join("openshell").join("gateways").join(name))
}

/// Deletes what OpenShell left of `gateway` under `config`, unless it's a local gateway with certificates of its own.
fn clear_folder(config: &Path, gateway: &GatewayInfo) -> std::io::Result<Forgotten> {
    let Some(folder) = gateway_folder(config, &gateway.name) else {
        return Ok(Forgotten::Clean);
    };
    if !folder.exists() {
        return Ok(Forgotten::Clean);
    }
    if gateway.has_certificates_of_its_own() {
        return Ok(Forgotten::Left(folder));
    }
    std::fs::remove_dir_all(&folder)?;
    Ok(Forgotten::Clean)
}

/// Where OpenShell keeps its configuration: `$XDG_CONFIG_HOME`, or `.config` in the home folder.
pub(super) fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

impl OpenShell {
    /// Registers `new`. OpenShell makes the one it adds the active gateway, which changes where `openshell` goes in a
    /// terminal, so whichever was active is made active again; Playpen names the gateway it wants in each call.
    /// `https://` opens a browser and waits for the sign-in.
    pub fn add_gateway(&self, new: &NewGateway) -> Result<(), OpenShellError> {
        let command = self.without_gateway();
        let before = command
            .gateways()
            .ok()
            .and_then(|all| all.into_iter().find(|gateway| gateway.active))
            .map(|gateway| gateway.name);
        let args = new.args();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let added = command.run(&format!("add {}", new.endpoint), &args);
        // Whether or not it was added, since a sign-in that was given up on may have registered it.
        if let Some(name) = before {
            let _ = command.run(
                "go back to the active gateway",
                &["gateway", "select", &name],
            );
        }
        added.map(drop)
    }

    /// Signs in to `gateway` again, in a browser, which it waits for.
    pub fn sign_in_to_gateway(&self, gateway: &GatewayInfo) -> Result<(), OpenShellError> {
        self.without_gateway()
            .run(
                &format!("sign in to “{}”", gateway.name),
                &["gateway", "login", &gateway.name],
            )
            .map(drop)
    }

    /// Forgets `gateway`, which leaves the gateway itself running. OpenShell leaves its folder, with the certificates
    /// in it; that goes too, unless the gateway says it's on this Mac.
    pub fn remove_gateway(&self, gateway: &GatewayInfo) -> Result<Forgotten, OpenShellError> {
        self.without_gateway().run(
            &format!("remove “{}”", gateway.name),
            &["gateway", "remove", &gateway.name],
        )?;
        let Some(config) = config_home() else {
            return Ok(Forgotten::Clean);
        };
        clear_folder(&config, gateway).map_err(|error| {
            OpenShellError::Failed(
                format!("delete what's left of “{}”", gateway.name),
                error.to_string(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        Forgotten, NewGateway, NewGatewayError, Oidc, Reach, clear_folder, gateway_folder,
        is_gateway_name,
    };
    use crate::openshell::GatewayInfo;

    #[test]
    fn an_address_says_how_the_gateway_is_reached() {
        let reach = |address: &str| NewGateway::new(address, "").map(|new| new.reach());
        assert_eq!(reach("https://os.example.com"), Ok(Reach::Browser));
        assert_eq!(reach("ssh://sam@10.0.0.5:8080"), Ok(Reach::Ssh));
        assert_eq!(reach("HTTP://127.0.0.1:8080"), Ok(Reach::PlainHttp));
        assert_eq!(reach("  https://os.example.com  "), Ok(Reach::Browser));
        assert_eq!(reach(""), Err(NewGatewayError::NoAddress));
        assert_eq!(
            reach("os.example.com"),
            Err(NewGatewayError::Scheme("os.example.com".to_string()))
        );
        assert!(reach("ftp://os.example.com").is_err());
    }

    #[test]
    fn a_gateway_is_added_with_its_name_when_one_is_given() {
        let named = NewGateway::new("https://os.example.com", "prod-eks").unwrap();
        assert_eq!(
            named.args(),
            [
                "gateway",
                "add",
                "https://os.example.com",
                "--name",
                "prod-eks"
            ]
        );
        let unnamed = NewGateway::new("https://os.example.com", "  ").unwrap();
        assert_eq!(unnamed.args(), ["gateway", "add", "https://os.example.com"]);
    }

    #[test]
    fn a_gateway_that_signs_in_with_oidc_is_added_with_its_issuer_and_only_what_was_given() {
        let new = NewGateway::new("https://os.example.com", "prod").unwrap();
        let only_issuer = new
            .clone()
            .with_oidc(Oidc {
                issuer: " https://login.example.com/realms/os ".to_string(),
                ..Oidc::default()
            })
            .unwrap();
        assert_eq!(
            only_issuer.args(),
            [
                "gateway",
                "add",
                "https://os.example.com",
                "--name",
                "prod",
                "--oidc-issuer",
                "https://login.example.com/realms/os"
            ],
            "the client is OpenShell's to choose when none is given"
        );
        let all = new
            .with_oidc(Oidc {
                issuer: "https://login.example.com".to_string(),
                client_id: "playpen".to_string(),
                audience: "openshell-api".to_string(),
                scopes: "  openid   profile ".to_string(),
            })
            .unwrap();
        assert_eq!(
            all.args()[5..],
            [
                "--oidc-issuer",
                "https://login.example.com",
                "--oidc-client-id",
                "playpen",
                "--oidc-audience",
                "openshell-api",
                "--oidc-scopes",
                "openid profile"
            ]
        );
    }

    #[test]
    fn oidc_needs_an_issuer_that_is_https_and_a_gateway_that_is_too() {
        let with = |endpoint: &str, issuer: &str| {
            NewGateway::new(endpoint, "").unwrap().with_oidc(Oidc {
                issuer: issuer.to_string(),
                ..Oidc::default()
            })
        };
        assert_eq!(
            with("https://os.example.com", "  "),
            Err(NewGatewayError::NoIssuer)
        );
        assert_eq!(
            with("https://os.example.com", "http://login.example.com"),
            Err(NewGatewayError::Issuer(
                "http://login.example.com".to_string()
            ))
        );
        assert_eq!(
            with("ssh://sam@host", "https://login.example.com"),
            Err(NewGatewayError::OidcNeedsHttps)
        );
        assert_eq!(
            with("http://127.0.0.1:8080", "https://login.example.com"),
            Err(NewGatewayError::OidcNeedsHttps)
        );
    }

    #[test]
    fn a_name_that_could_leave_its_folder_is_refused() {
        for bad in [
            "../etc",
            "a/b",
            ".hidden",
            "..",
            "with space",
            "a\\b",
            &"x".repeat(64),
        ] {
            assert!(!is_gateway_name(bad), "{bad}");
            assert_eq!(
                NewGateway::new("https://os.example.com", bad),
                Err(NewGatewayError::Name(bad.to_string()))
            );
        }
        assert!(is_gateway_name("prod-eks_2.test"));
        assert_eq!(gateway_folder(std::path::Path::new("/c"), "../x"), None);
    }

    fn gateway(name: &str, kind: &str, remote: bool, auth: &str) -> GatewayInfo {
        GatewayInfo {
            name: name.to_string(),
            endpoint: "https://os.example.com".to_string(),
            active: false,
            kind: kind.to_string(),
            is_remote: remote,
            auth: auth.to_string(),
        }
    }

    fn folder_with_certificates(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let config = std::env::temp_dir().join(format!("gateways-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&config);
        let mtls = config.join("openshell/gateways").join(name).join("mtls");
        fs::create_dir_all(&mtls).unwrap();
        fs::write(mtls.join("tls.key"), "a key").unwrap();
        (config, mtls)
    }

    #[test]
    fn what_is_left_of_a_remote_gateway_is_deleted_with_its_certificates() {
        let (config, mtls) = folder_with_certificates("prod");
        let outcome = clear_folder(&config, &gateway("prod", "remote", true, "mtls")).unwrap();
        assert_eq!(outcome, Forgotten::Clean);
        assert!(!mtls.exists());
        // Nothing there is nothing to delete.
        assert_eq!(
            clear_folder(&config, &gateway("prod", "remote", true, "mtls")).unwrap(),
            Forgotten::Clean
        );
        let _ = fs::remove_dir_all(&config);
    }

    #[test]
    fn what_is_left_of_a_local_gateway_with_certificates_of_its_own_is_kept() {
        let (config, mtls) = folder_with_certificates("openshell");
        let outcome = clear_folder(&config, &gateway("openshell", "local", false, "mtls")).unwrap();
        assert!(
            matches!(outcome, Forgotten::Left(folder) if folder.ends_with("gateways/openshell"))
        );
        assert!(mtls.join("tls.key").exists());
        let _ = fs::remove_dir_all(&config);
    }

    #[test]
    fn a_local_gateway_without_certificates_is_not_mistaken_for_one_that_has_them() {
        // OpenShell calls a plain `http://` gateway `local` too, with `plaintext` for its authentication.
        let (config, mtls) = folder_with_certificates("dev");
        let plain = gateway("dev", "local", false, "plaintext");
        assert!(plain.is_local() && !plain.has_certificates_of_its_own());
        assert_eq!(clear_folder(&config, &plain).unwrap(), Forgotten::Clean);
        assert!(!mtls.exists());
        let _ = fs::remove_dir_all(&config);
    }
}
