//! Providers, a place in the main window: the model keys OpenShell holds for sandboxes, added, given new keys and deleted here. Every
//! call to OpenShell runs off the window's thread. A key goes from its field to OpenShell and nowhere else: it isn't
//! kept, logged or shown again.

use std::path::PathBuf;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::component::{Icon, IndexPath};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::radio::Radio;
use playpen_core::{
    Access, Account, ChatGptError, DeviceCode, Endpoint, Gateway, Key, KeySource, Kind, Login,
    NewProvider, NewService, OpenShell, Provider, ProviderError, ServiceAuth, ServiceCatalog,
    ServiceCategory, TokenSignIn, find_claude, find_codex, find_op,
};
use playpen_core::{
    KeepKeysIn, KeyHome, KeyHomeError, KeyHomes, keychain_service, save_in_1password,
    save_in_keychain,
};
use rust_i18n::t;
use typecase::{Toast, shell};

use crate::shortcuts::{DismissForm, FORM_CONTEXT};
use crate::view::{choice_chip, copyable_command};

/// Events `Providers` emits to the main view, e.g. when a new service is added so it can be attached to the sandbox
/// that asked for it.
#[derive(Debug, Clone)]
pub enum ProvidersEvent {
    ServiceAdded(String),
    ServiceCancelled,
}

impl EventEmitter<ProvidersEvent> for Providers {}

/// The gateway the main window shows, which a provider is for: OpenShell keeps each in the gateway that holds it, so
/// Settings acts on the one whose sandboxes are in front of the person.
#[derive(Debug, Clone, Default)]
pub struct ShownGateway(pub Gateway);

impl Global for ShownGateway {}

/// The providers part of Settings.
pub struct Providers {
    /// The gateway it acts on: the main window's at first, then the one whose group was last acted in.
    gateway: Gateway,
    /// Each gateway's providers, by its name, in OpenShell's order, or why the gateways couldn't be listed.
    listings: Result<Vec<(String, Listing)>, String>,
    form: Option<Form>,
    service: Option<ServiceForm>,
    /// Every gateway's name, so a provider can be added to another.
    gateways: Vec<String>,
    /// Where each provider's key lives besides OpenShell, when it's kept anywhere.
    homes: KeyHomes,
    /// The last thing done that failed, until something else is done.
    failed: Option<String>,
    /// Whatever is being asked of OpenShell, ended with the pane.
    _working: Option<Task<()>>,
}

enum Listing {
    Loading,
    Listed(Vec<Provider>),
    Failed(String),
}

/// Adding a provider, or giving one a new key.
struct Form {
    /// The provider being given a new key, or none for a new one.
    replacing: Option<Provider>,
    kind: Kind,
    /// The kinds to choose from, narrowed as the person types.
    kinds: Entity<SelectState<SearchableVec<KindChoice>>>,
    name: Entity<InputState>,
    address: Entity<InputState>,
    key: Entity<InputState>,
    /// The name Playpen last suggested, which choosing another kind changes while the person hasn't typed one.
    suggested: String,
    saving: bool,
    /// Whether it's waiting for the person to sign in.
    signing_in: bool,
    /// The page and code to sign in with, once Codex has given them.
    device: Option<DeviceCode>,
    /// Whether the key is a 1Password reference rather than pasted.
    from_1password: bool,
    reference: Entity<InputState>,
    /// Where a pasted key is kept besides OpenShell.
    keep: KeepKeysIn,
    /// The provider being put on another gateway, and that gateway, when it's that rather than a new provider.
    copying: Option<(Provider, Gateway)>,
    /// Signing in with Claude for a token, while it's done.
    claude: Option<ClaudeSignIn>,
    /// Whether the key was filled in from signing in with Claude.
    from_claude: bool,
    _chose: Subscription,
}

/// `claude setup-token`, run by Playpen, waiting for the person to sign in. Dropping it ends it.
struct ClaudeSignIn {
    session: Arc<Mutex<TokenSignIn>>,
    /// The page to sign in at, once Claude Code has given it.
    link: Option<String>,
    /// The code the sign-in page shows, if it shows one, to give Claude Code.
    code: Entity<InputState>,
    /// Whether the code has gone to Claude Code, which is swapping it for a token.
    sent: bool,
    _waiting: Task<()>,
}

/// Adding a service: pick from the bundled curated services JSON (or a shared JSON catalog) or fill in a custom
/// host and variable.
struct ServiceForm {
    catalog: ServiceCatalog,
    catalog_errors: Vec<String>,
    search: Entity<InputState>,
    category: Option<ServiceCategory>,
    /// The preset currently applied, if any.
    chosen_preset: Option<String>,
    /// Whether the advanced endpoint/header/access fields are expanded when a preset is active.
    customize: bool,
    name: Entity<InputState>,
    address: Entity<InputState>,
    extra_endpoints: Vec<Endpoint>,
    variable: Entity<InputState>,
    auth_header: bool,
    header: Entity<InputState>,
    access: Access,
    from: From,
    key: Entity<InputState>,
    reference: Entity<InputState>,
    environment: Entity<InputState>,
    keychain: Entity<InputState>,
    /// Optional Keychain account name paired with `keychain` (such as Patchbay's `gateway/token`).
    keychain_account: Option<String>,
    /// Where to create a key for the chosen preset.
    help: Option<String>,
    /// Extra guidance for the chosen preset (e.g. Patchbay, GCP, AWS Bedrock, Datadog).
    preset_note: Option<String>,
    /// If opened from a specific sandbox's Services row, that sandbox's name so Playpen attaches it only there.
    for_sandbox: Option<String>,
    saving: bool,
    _subscriptions: Vec<Subscription>,
}

/// Where a service's key comes from, as the form offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum From {
    Pasted,
    Reference,
    Environment,
    Keychain,
}

/// A kind of provider, as the Kind menu lists it.
#[derive(Clone)]
struct KindChoice(Kind);

impl SelectItem for KindChoice {
    type Value = Kind;

    fn title(&self) -> SharedString {
        self.0.name().into()
    }

    fn value(&self) -> &Kind {
        &self.0
    }
}

impl Providers {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            gateway: Gateway::Active,
            listings: Ok(Vec::new()),
            form: None,
            service: None,
            gateways: Vec::new(),
            homes: KeyHomes::default(),
            failed: None,
            _working: None,
        };
        this.refresh(window, cx);
        this
    }

    /// Total number of listed providers across gateways, once loaded.
    pub fn total_count(&self) -> Option<usize> {
        let listings = self.listings.as_ref().ok()?;
        let mut loaded = false;
        let mut total = 0;
        for (_, listing) in listings {
            if let Listing::Listed(providers) = listing {
                loaded = true;
                total += providers.len();
            }
        }
        loaded.then_some(total)
    }

    /// Dismisses the active Add Provider or Add Service form on Escape when not mid-save.
    fn dismiss_form(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = &self.form {
            if !form.saving || form.signing_in {
                self.cancel(cx);
            }
        } else if let Some(service) = &self.service
            && !service.saving
        {
            self.cancel_service(cx);
        }
    }

    fn cancel_service(&mut self, cx: &mut Context<Self>) {
        let had_target = self
            .service
            .as_ref()
            .and_then(|service| service.for_sandbox.as_ref())
            .is_some();
        self.service = None;
        self.failed = None;
        if had_target {
            cx.emit(ProvidersEvent::ServiceCancelled);
        }
        cx.notify();
    }

    /// Asks OpenShell for its providers again.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gateway = cx
            .try_global::<ShownGateway>()
            .map(|shown| shown.0.clone())
            .unwrap_or_default();
        if cfg!(feature = "screenshot") {
            // Shots show stand-ins rather than anyone's real providers, and `PLAYPEN_SHOT_ADD=litellm` the form.
            let stand_in = |name: &str, kind: &str| Provider {
                name: name.to_string(),
                kind: kind.to_string(),
            };
            self.gateway = Gateway::named("openshell");
            self.gateways = vec!["openshell".to_string(), "microvm".to_string()];
            self.listings = Ok(vec![
                (
                    "openshell".to_string(),
                    Listing::Listed(vec![
                        stand_in("claude-subscription", "claude-code-oauth"),
                        stand_in("openrouter", "playpen-openrouter"),
                        stand_in("work", "playpen-litellm-work"),
                        stand_in("github", "playpen-service-github"),
                    ]),
                ),
                (
                    "microvm".to_string(),
                    Listing::Listed(vec![stand_in("claude-subscription", "claude-code-oauth")]),
                ),
            ]);
            if std::env::var("PLAYPEN_SHOT_ADD").as_deref() == Ok("service") {
                self.start_adding_service(window, cx);
            } else if let Ok(kind) = std::env::var("PLAYPEN_SHOT_ADD") {
                let kind = Kind::ALL
                    .into_iter()
                    .find(|known| known.suggested_name() == kind)
                    .unwrap_or(Kind::OpenRouter);
                self.open_form(None, kind, window, cx);
            }
            return;
        }
        if let Some(path) = crate::settings::file("key-homes.json") {
            self.homes = KeyHomes::load(&path);
        }
        if let Ok(listings) = &mut self.listings {
            for (_, listing) in listings.iter_mut() {
                *listing = Listing::Loading;
            }
        }
        self._working = Some(cx.spawn_in(window, async move |this, cx| {
            // Every gateway's, one after another; one that isn't answering says so in its own group.
            let listed = cx
                .background_spawn(async move {
                    let openshell = OpenShell::find().map_err(|error| error.to_string())?;
                    let gateways = openshell.gateways().map_err(|error| error.to_string())?;
                    let active = gateways
                        .iter()
                        .find(|gateway| gateway.active)
                        .map(|gateway| gateway.name.clone());
                    let listings = gateways
                        .into_iter()
                        .map(|gateway| {
                            let listing = match openshell
                                .clone()
                                .on(Gateway::named(gateway.name.clone()))
                                .providers()
                            {
                                Ok(providers) => Listing::Listed(providers),
                                Err(error) => {
                                    tracing::warn!(gateway = %gateway.name, "couldn't list the providers: {error}");
                                    Listing::Failed(error.to_string())
                                }
                            };
                            (gateway.name, listing)
                        })
                        .collect::<Vec<_>>();
                    Ok::<_, String>((listings, active))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match listed {
                    Ok((listings, active)) => {
                        this.gateways = listings.iter().map(|(name, _)| name.clone()).collect();
                        // The active gateway by its name, so its group is the one acted in.
                        if this.gateway == Gateway::Active
                            && let Some(active) = active
                        {
                            this.gateway = Gateway::named(active);
                        }
                        this.listings = Ok(listings);
                    }
                    Err(error) => {
                        tracing::warn!("couldn't list the gateways: {error}");
                        this.listings = Err(error);
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Opens the form for a new service, with the bundled common services catalog ready to pick from.
    pub(crate) fn start_adding_service(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_adding_service_for(None, window, cx);
    }

    /// Opens the form for a new service, optionally scoped to attach to one sandbox once created.
    pub(crate) fn start_adding_service_for(
        &mut self,
        for_sandbox: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(shown) = cx.try_global::<ShownGateway>()
            && shown.0.name().is_some()
        {
            self.gateway = shown.0.clone();
        }
        let field = |placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            let placeholder = placeholder.to_string();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let dir = crate::settings::file("settings.toml")
            .and_then(|path| path.parent().map(PathBuf::from));
        let (catalog, catalog_errors) = ServiceCatalog::load(dir.as_deref());
        let search = field(
            "Search Patchbay, GitHub, Cloudflare, Statsig, GCP…",
            window,
            cx,
        );
        let keychain = field("Keychain item", window, cx);
        let subscriptions = vec![
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&keychain, |this, _, event: &InputEvent, _| {
                if matches!(event, InputEvent::Change)
                    && let Some(form) = &mut this.service
                {
                    form.keychain_account = None;
                }
            }),
        ];
        search.read(cx).focus_handle(cx).focus(window, cx);
        self.form = None;
        self.failed = None;
        self.service = Some(ServiceForm {
            catalog,
            catalog_errors,
            search,
            category: None,
            chosen_preset: None,
            customize: true,
            name: field("github", window, cx),
            address: field("api.github.com", window, cx),
            extra_endpoints: Vec::new(),
            variable: field("GITHUB_TOKEN", window, cx),
            auth_header: false,
            header: field("STATSIG-API-KEY", window, cx),
            access: Access::ReadOnly,
            from: if find_op().is_some() {
                From::Reference
            } else {
                From::Pasted
            },
            key: cx.new(|cx| InputState::new(window, cx).masked(true)),
            reference: field("op://Private/GitHub/token", window, cx),
            environment: field("Environment ID", window, cx),
            keychain,
            keychain_account: None,
            help: None,
            preset_note: None,
            for_sandbox,
            saving: false,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    /// Applies a service preset from the catalog into the form fields, leaving everything editable.
    fn apply_service_preset(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.service else {
            return;
        };
        let Some(preset) = form.catalog.find(id).cloned() else {
            return;
        };
        let host = preset.resolved_host();
        let auth = preset.service_auth();
        let extra = preset.extra_endpoints();
        form.chosen_preset = Some(preset.id.clone());
        form.customize = false;
        form.name.update(cx, |field, cx| {
            field.set_value(preset.id.clone(), window, cx)
        });
        form.address
            .update(cx, |field, cx| field.set_value(host, window, cx));
        form.extra_endpoints = extra;
        form.variable.update(cx, |field, cx| {
            field.set_value(preset.variable.clone(), window, cx)
        });
        match auth {
            ServiceAuth::Bearer => {
                form.auth_header = false;
                form.header
                    .update(cx, |field, cx| field.set_value("", window, cx));
            }
            ServiceAuth::Header(header) => {
                form.auth_header = true;
                form.header
                    .update(cx, |field, cx| field.set_value(header, window, cx));
            }
        }
        form.access = preset.access.into();
        form.help = preset.help.clone();
        form.preset_note = preset.note.clone();
        if let Some(service) = &preset.keychain_service {
            form.from = From::Keychain;
            form.keychain
                .update(cx, |field, cx| field.set_value(service.clone(), window, cx));
            // Set after `set_value` so the change subscription doesn't clear the preset's account.
            form.keychain_account = preset.keychain_account.clone();
        } else {
            form.keychain_account = None;
            let focus_field = match form.from {
                From::Pasted => &form.key,
                From::Reference => &form.reference,
                From::Environment => &form.environment,
                From::Keychain => &form.keychain,
            };
            focus_field.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.failed = None;
        cx.notify();
    }

    /// Loads a shared service catalog JSON file into Playpen's Application Support `services/` folder.
    fn import_service_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = crate::settings::file("settings.toml")
            .and_then(|path| path.parent().map(PathBuf::from))
        else {
            return;
        };
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("Load Services JSON").into_owned().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let imported = ServiceCatalog::import_file(&dir, &path);
            let _ = this.update(cx, |this, cx| {
                match imported {
                    Ok(_) => {
                        this.failed = None;
                        if let Some(form) = &mut this.service {
                            let (catalog, errors) = ServiceCatalog::load(Some(&dir));
                            form.catalog = catalog;
                            form.catalog_errors = errors;
                        }
                    }
                    Err(error) => this.failed = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Exports the current service catalog as a shareable pretty-printed JSON file.
    fn export_service_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let json = match &self.service {
            Some(form) => form.catalog.export_json(&[]),
            None => ServiceCatalog::builtin().export_json(&[]),
        };
        let folder = std::env::var_os("HOME").map_or_else(
            || PathBuf::from("/"),
            |home| PathBuf::from(home).join("Desktop"),
        );
        let chosen = cx.prompt_for_new_path(&folder, Some("playpen-services.json"));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = chosen.await else {
                return;
            };
            let written = playpen_core::write_atomically(&path, json.as_bytes());
            if let Err(error) = written {
                let _ = this.update(cx, |this, cx| {
                    this.failed = Some(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Adds the service, off the window's thread. A 1Password or Keychain secret goes straight to OpenShell, never
    /// through here.
    fn save_service(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.service else {
            return;
        };
        if form.saving {
            return;
        }
        let auth = if form.auth_header {
            let header = form.header.read(cx).value().trim().to_string();
            if header.is_empty() {
                self.failed = Some(t!("Give the header name for this service's key.").into_owned());
                cx.notify();
                return;
            }
            ServiceAuth::Header(header)
        } else {
            ServiceAuth::Bearer
        };
        let new = match NewService::new(
            &form.name.read(cx).value(),
            &form.address.read(cx).value(),
            &form.variable.read(cx).value(),
            form.access,
        ) {
            Ok(new) => new
                .with_auth(auth)
                .with_extra_endpoints(form.extra_endpoints.clone()),
            Err(error) => {
                self.failed = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let source = match form.from {
            From::Pasted => KeySource::Key(Key::new(&form.key.read(cx).value())),
            From::Reference => KeySource::Reference(form.reference.read(cx).value().to_string()),
            From::Environment => {
                KeySource::Environment(form.environment.read(cx).value().to_string())
            }
            From::Keychain => {
                let service = form.keychain.read(cx).value().to_string();
                match &form.keychain_account {
                    Some(account) => KeySource::KeychainAccount {
                        service,
                        account: account.clone(),
                    },
                    None => KeySource::Keychain(service),
                }
            }
        };
        form.saving = true;
        self.failed = None;
        // On the gateway shown, as every other provider is.
        let gateway = self.gateway.clone();
        let saved = cx.background_spawn(async move {
            OpenShell::find()?
                .on(gateway)
                .add_service(&new, &source, find_op().as_deref())?;
            anyhow::Ok(new.name)
        });
        self._working = Some(cx.spawn_in(window, async move |this, cx| {
            let saved = saved.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match saved {
                    Ok(name) => {
                        tracing::info!(service = %name, "added a service");
                        let for_sandbox = this
                            .service
                            .as_ref()
                            .and_then(|service| service.for_sandbox.clone());
                        if for_sandbox.is_none() {
                            Toast::new(t!("Added service “%{name}”", name = name).into_owned())
                                .key("provider-saved")
                                .show(window, cx);
                        }
                        this.service = None;
                        this.refresh(window, cx);
                        cx.emit(ProvidersEvent::ServiceAdded(name));
                    }
                    Err(error) => {
                        tracing::warn!("couldn't add a service: {error:#}");
                        if let Some(form) = &mut this.service {
                            form.saving = false;
                        }
                        this.failed = Some(format!("{error:#}"));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn service_form(&self, form: &ServiceForm, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let has_op = find_op().is_some();
        let query = form.search.read(cx).value().trim().to_string();
        let presets = form.catalog.filter(&query, form.category);
        let category_chip =
            |id: &'static str, label: String, active: bool, category: Option<ServiceCategory>| {
                choice_chip(active, cx)
                    .id(id)
                    .child(label)
                    .when(!form.saving, |chip| {
                        chip.on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(form) = &mut this.service {
                                form.category = category;
                                cx.notify();
                            }
                        }))
                    })
            };
        let auth_choice = |custom: bool, label: std::borrow::Cow<str>, id: &'static str| {
            Radio::new(id)
                .label(label.into_owned())
                .checked(form.auth_header == custom)
                .disabled(form.saving)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = &mut this.service {
                        form.auth_header = custom;
                        cx.notify();
                    }
                }))
        };
        let access = |access: Access, label: std::borrow::Cow<str>, id: &'static str| {
            Radio::new(id)
                .label(label.into_owned())
                .checked(form.access == access)
                .disabled(form.saving)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = &mut this.service {
                        form.access = access;
                        cx.notify();
                    }
                }))
        };
        let from = |from: From, label: std::borrow::Cow<str>, id: &'static str| {
            Radio::new(id)
                .label(label.into_owned())
                .checked(form.from == from)
                .disabled(
                    form.saving || (matches!(from, From::Reference | From::Environment) && !has_op),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = &mut this.service {
                        form.from = from;
                        cx.notify();
                    }
                }))
        };
        let (key_field, key_note) = match form.from {
            From::Pasted => (&form.key, t!("OpenShell keeps it; Playpen doesn't.")),
            From::Reference => (
                &form.reference,
                t!(
                    "In 1Password, open a field's menu and choose Copy Secret Reference. The 1Password CLI reads it and hands it to OpenShell."
                ),
            ),
            From::Keychain => (
                &form.keychain,
                match &form.keychain_account {
                    Some(account) => t!(
                        "Reads the Keychain item for account “%{account}”. macOS asks once whether reading it is allowed.",
                        account = account
                    ),
                    None => t!(
                        "The item's name, as Keychain Access shows it under Where. macOS asks once whether reading it is allowed."
                    ),
                },
            ),
            From::Environment => (
                &form.environment,
                t!(
                    "From the Environment's Manage environment ▸ Copy environment ID. It needs a secret named as the variable."
                ),
            ),
        };
        let preset_list = v_flex()
            .gap_2()
            .when(!form.catalog_errors.is_empty(), |box_| {
                box_.children(form.catalog_errors.iter().map(|error| {
                    div()
                        .text_sm()
                        .text_color(theme.warning)
                        .child(error.clone())
                }))
            })
            .child(small_field(&form.search))
            .child(
                h_flex()
                    .gap_1p5()
                    .flex_wrap()
                    .child(category_chip(
                        "svc-cat-all",
                        t!("All").into_owned(),
                        form.category.is_none(),
                        None,
                    ))
                    .children(ServiceCategory::ALL.into_iter().map(|category| {
                        let id = match category {
                            ServiceCategory::Gateway => "svc-cat-gateway",
                            ServiceCategory::Cloud => "svc-cat-cloud",
                            ServiceCategory::Code => "svc-cat-code",
                            ServiceCategory::Observability => "svc-cat-observability",
                            ServiceCategory::Databases => "svc-cat-databases",
                            ServiceCategory::Productivity => "svc-cat-productivity",
                            ServiceCategory::Packages => "svc-cat-packages",
                        };
                        category_chip(
                            id,
                            t!(category.label()).into_owned(),
                            form.category == Some(category),
                            Some(category),
                        )
                    })),
            )
            .child(
                div()
                    .id("service-presets")
                    .max_h(rems(13.))
                    .overflow_y_scroll()
                    .rounded(theme.radius * 1.5)
                    .border_1()
                    .border_color(theme.border)
                    .bg(shell::recessed_colour(theme))
                    .p_1p5()
                    .child(
                        v_flex()
                            .gap_1()
                            .when(presets.is_empty(), |list| {
                                list.child(div().p_2().child(note(
                                    t!("No bundled services match. Fill in the fields below for any custom host."),
                                    cx,
                                )))
                            })
                            .children(presets.into_iter().map(|preset| {
                                let id = preset.id.clone();
                                let selected =
                                    form.chosen_preset.as_deref() == Some(preset.id.as_str());
                                let hover = theme.list_hover;
                                let host_label = if preset.extra_hosts.is_empty() {
                                    preset.resolved_host()
                                } else {
                                    t!(
                                        "%{host} +%{more} more",
                                        host = preset.resolved_host(),
                                        more = preset.extra_hosts.len()
                                    )
                                    .into_owned()
                                };
                                h_flex()
                                    .id(SharedString::from(format!("preset-{id}")))
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .px_2()
                                    .py_1p5()
                                    .rounded(theme.radius)
                                    .cursor_pointer()
                                    .when(selected, |row| row.bg(theme.list_active))
                                    .when(!selected, |row| row.hover(move |row| row.bg(hover)))
                                    .on_click(cx.listener({
                                        let id = id.clone();
                                        move |this, _, window, cx| {
                                            this.apply_service_preset(&id, window, cx);
                                        }
                                    }))
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                h_flex()
                                                    .gap_1p5()
                                                    .items_center()
                                                    .flex_wrap()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_medium()
                                                            .child(preset.name.clone()),
                                                    )
                                                    .when_some(
                                                        preset.source.clone(),
                                                        |row, source| {
                                                            row.child(
                                                                shell::badge(Tag::secondary())
                                                                    .xsmall()
                                                                    .child(source),
                                                            )
                                                        },
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .truncate()
                                                    .child(preset.summary.clone()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .child(shell::code_chip(host_label, cx)),
                                    )
                            })),
                    ),
            );

        let selected_summary = form.chosen_preset.as_ref().map(|_| {
            let name = form.name.read(cx).value().to_string();
            let host = form.address.read(cx).value().to_string();
            let var = form.variable.read(cx).value().to_string();
            let access_words = match form.access {
                Access::ReadOnly => t!("Read only"),
                Access::ReadWrite => t!("Read and write"),
            };
            h_flex()
                .justify_between()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .rounded(theme.radius * 1.5)
                .border_1()
                .border_color(theme.border)
                .bg(shell::recessed_colour(theme))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1p5()
                        .items_center()
                        .min_w_0()
                        .child(div().text_sm().font_medium().child(name))
                        .child(shell::code_chip(host, cx))
                        .child(shell::code_chip(var, cx))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(access_words.into_owned()),
                        ),
                )
                .child(
                    Button::new("toggle-customize-service")
                        .ghost()
                        .xsmall()
                        .label(if form.customize {
                            t!("Hide Details").into_owned()
                        } else {
                            t!("Customize").into_owned()
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(form) = &mut this.service {
                                form.customize = !form.customize;
                                cx.notify();
                            }
                        })),
                )
        });

        let gateway_label = self
            .gateway
            .name()
            .map(|gw| t!("Add a Service · %{gateway}", gateway = gw).into_owned())
            .unwrap_or_else(|| t!("Add a Service").into_owned());
        let header = h_flex()
            .flex_none()
            .justify_between()
            .items_center()
            .gap_2()
            .px_4()
            .h(rems(38. / 13.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .min_w_0()
                    .child(
                        Button::new("back-from-service")
                            .ghost()
                            .xsmall()
                            .disabled(form.saving)
                            .icon(assets::IconName::ArrowLeft)
                            .tooltip(t!("Back").into_owned())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_service(cx);
                            })),
                    )
                    .child(div().truncate().font_semibold().child(gateway_label)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("import-services-json")
                            .ghost()
                            .xsmall()
                            .disabled(form.saving)
                            .label(t!("Load JSON…").into_owned())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.import_service_catalog(window, cx);
                            })),
                    )
                    .child(
                        Button::new("export-services-json")
                            .ghost()
                            .xsmall()
                            .disabled(form.saving)
                            .label(t!("Export JSON…").into_owned())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.export_service_catalog(window, cx);
                            })),
                    ),
            );

        let body = v_flex()
            .gap_4()
            .p_4()
            .when_some(form.for_sandbox.clone(), |this, sandbox| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded(theme.radius)
                        .border_1()
                        .border_color(theme.info.opacity(0.4))
                        .bg(shell::recessed_colour(theme))
                        .child(
                            Icon::new(assets::IconName::Box)
                                .small()
                                .flex_none()
                                .text_color(theme.info),
                        )
                        .child(note(
                            t!(
                                "Once added, this service will be attached only to “%{sandbox}” — other sandboxes on this gateway won't have access.",
                                sandbox = sandbox
                            ),
                            cx,
                        )),
                )
            })
            .child(field(t!("Common services"), preset_list, cx))
            .when_some(selected_summary, |this, summary| this.child(summary))
            .when_some(form.preset_note.clone(), |this, text| {
                this.child(
                    div()
                        .max_w(rems(42.))
                        .p_2p5()
                        .rounded(theme.radius)
                        .bg(shell::recessed_colour(theme))
                        .border_1()
                        .border_color(shell::hairline(theme))
                        .child(note(text.into(), cx)),
                )
            })
            .child(field(
                t!("Key from"),
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_4()
                            .child(from(
                                From::Reference,
                                t!("1Password reference"),
                                "from-reference",
                            ))
                            .child(from(
                                From::Environment,
                                t!("1Password Environment"),
                                "from-environment",
                            ))
                            .child(from(From::Keychain, t!("Keychain"), "from-keychain"))
                            .child(from(From::Pasted, t!("Paste"), "from-pasted")),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().w(rems(22.)).child(small_field(key_field)))
                            .when_some(form.help.clone(), |row, url| {
                                row.child(
                                    Button::new("service-help")
                                        .link()
                                        .xsmall()
                                        .label(t!("Get a key").into_owned())
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                )
                            }),
                    )
                    .child(note(key_note, cx))
                    .when(!has_op, |this| {
                        this.child(
                            v_flex()
                                .gap_1()
                                .items_start()
                                .child(note(
                                    t!("Install the 1Password CLI to take keys from 1Password:"),
                                    cx,
                                ))
                                .child(copyable_command(
                                    "copy-op-install-service",
                                    "brew install 1password-cli",
                                    cx,
                                )),
                        )
                    }),
                cx,
            ))
            .when(form.customize, |this| {
                this.child(
                    v_flex()
                        .gap_4()
                        .pt_3()
                        .border_t_1()
                        .border_color(shell::hairline(theme))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_4()
                                .items_start()
                                .child(field(
                                    t!("Name"),
                                    div().w(rems(15.)).child(small_field(&form.name)),
                                    cx,
                                ))
                                .child(field(
                                    t!("Host"),
                                    v_flex()
                                        .gap_1()
                                        .child(div().w(rems(20.)).child(small_field(&form.address)))
                                        .when(!form.extra_endpoints.is_empty(), |col| {
                                            col.child(h_flex().gap_1().flex_wrap().children(
                                                form.extra_endpoints.iter().map(|endpoint| {
                                                    shell::code_chip(endpoint.host.clone(), cx)
                                                }),
                                            ))
                                        })
                                        .child(note(
                                            if form.extra_endpoints.is_empty() {
                                                t!("The only host its key is sent to.")
                                            } else {
                                                t!("The only hosts its key is sent to.")
                                            },
                                            cx,
                                        )),
                                    cx,
                                )),
                        )
                        .child(field(
                            t!("Variable"),
                            v_flex()
                                .gap_1()
                                .child(div().w(rems(16.)).child(small_field(&form.variable)))
                                .child(
                                    h_flex()
                                        .gap_4()
                                        .child(auth_choice(false, t!("Bearer token"), "auth-bearer"))
                                        .child(auth_choice(true, t!("Custom header"), "auth-header")),
                                )
                                .when(form.auth_header, |col| {
                                    col.child(div().w(rems(16.)).child(small_field(&form.header)))
                                })
                                .child(note(
                                    if form.auth_header {
                                        t!("Where programs in the sandbox find its stand-in, sent in the header named above.")
                                    } else {
                                        t!("Where programs in the sandbox find its stand-in, sent as a bearer token.")
                                    },
                                    cx,
                                )),
                            cx,
                        ))
                        .child(field(
                            t!("Access"),
                            h_flex()
                                .gap_4()
                                .child(access(Access::ReadOnly, t!("Read only"), "access-read"))
                                .child(access(
                                    Access::ReadWrite,
                                    t!("Read and write"),
                                    "access-write",
                                )),
                            cx,
                        )),
                )
            });

        let footer = h_flex()
            .flex_none()
            .justify_between()
            .items_center()
            .gap_3()
            .px_4()
            .py_2p5()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when_some(self.failed.clone(), |slot, failed| {
                        slot.child(div().text_sm().text_color(theme.danger).child(failed))
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(
                        Button::new("cancel-service")
                            .ghost()
                            .small()
                            .disabled(form.saving)
                            .label(t!("Cancel").into_owned())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_service(cx);
                            })),
                    )
                    .child(
                        Button::new("save-service")
                            .primary()
                            .small()
                            .loading(form.saving)
                            .label(if form.for_sandbox.is_some() {
                                t!("Add and Attach").into_owned()
                            } else {
                                t!("Add Service").into_owned()
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_service(window, cx);
                            })),
                    ),
            );

        v_flex()
            .size_full()
            .key_context(FORM_CONTEXT)
            .on_action(cx.listener(|this, _: &DismissForm, _, cx| {
                this.dismiss_form(cx);
            }))
            .child(header)
            .child(
                div()
                    .id("service-form-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(footer)
            .into_any_element()
    }

    fn start_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_form(None, Kind::OpenRouter, window, cx);
    }

    fn start_replacing(&mut self, provider: Provider, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = provider.playpen_kind() else {
            return;
        };
        self.open_form(Some(provider), kind, window, cx);
    }

    /// Opens the form for a new provider of `kind`, as Add Provider… does with it chosen.
    pub(crate) fn add(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.open_form(None, kind, window, cx);
        cx.notify();
    }

    fn open_form(
        &mut self,
        replacing: Option<Provider>,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.service = None;
        let suggested = kind.suggested_name().to_string();
        let name = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(suggested.clone(), window, cx);
            state
        });
        let address =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://litellm.example.com"));
        let key = cx.new(|cx| InputState::new(window, cx).masked(true));
        let reference = cx
            .new(|cx| InputState::new(window, cx).placeholder("op://Private/Fireworks/credential"));
        let settings = crate::settings::current(cx);
        // 1Password only when it can be saved in: `op` is installed and a vault is chosen.
        let keep = match settings.keep_keys_in {
            KeepKeysIn::OnePassword if find_op().is_none() || settings.vault.is_none() => {
                KeepKeysIn::Nowhere
            }
            keep => keep,
        };
        let chosen = Kind::ALL.iter().position(|known| *known == kind);
        let kinds = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(Kind::ALL.map(KindChoice).to_vec()),
                chosen.map(IndexPath::new),
                window,
                cx,
            )
            .searchable(true)
        });
        let chose = cx.subscribe_in(
            &kinds,
            window,
            |this, _, event: &SelectEvent<_>, window, cx| {
                let SelectEvent::Confirm(Some(kind)) = event else {
                    return;
                };
                this.choose_kind(*kind, window, cx);
            },
        );
        let first = if replacing.is_some() { &key } else { &name };
        first.read(cx).focus_handle(cx).focus(window, cx);
        self.form = Some(Form {
            replacing,
            kind,
            kinds,
            name,
            address,
            key,
            suggested,
            saving: false,
            signing_in: false,
            device: None,
            from_1password: false,
            reference,
            keep,
            copying: None,
            claude: None,
            from_claude: false,
            _chose: chose,
        });
        self.failed = None;
        cx.notify();
    }

    fn choose_kind(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        let typed = form.name.read(cx).value().trim().to_string();
        if typed == form.suggested || typed.is_empty() {
            let suggested = kind.suggested_name().to_string();
            form.name
                .update(cx, |name, cx| name.set_value(suggested.clone(), window, cx));
            form.suggested = suggested;
        }
        form.kind = kind;
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.form.as_ref().is_some_and(|form| form.signing_in) {
            // Ending the wait ends the sign-in, and deletes its folder.
            self._working = None;
        }
        self.form = None;
        self.failed = None;
        cx.notify();
    }

    /// Adds the provider, or gives it its new key, off the window's thread. A ChatGPT subscription signs in instead.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        if form.saving {
            return;
        }
        let job = match (&form.copying, &form.replacing) {
            (Some((provider, to)), _) => Job::Copy(provider.clone(), to.clone()),
            (None, Some(provider)) => Job::Replace(provider.clone()),
            (None, None) => match NewProvider::new(
                form.kind,
                &form.name.read(cx).value(),
                &form.address.read(cx).value(),
            ) {
                Ok(new) => Job::Add(new),
                Err(error) => {
                    self.failed = Some(error.to_string());
                    cx.notify();
                    return;
                }
            },
        };
        if form.kind.signs_in() {
            self.sign_in(job, window, cx);
            return;
        }
        // Where the key comes from, and where it's kept besides OpenShell: a reference is 1Password's already, and a
        // pasted key is saved where the form says first, so OpenShell takes it from there.
        let from = if form.from_1password && !matches!(job, Job::Replace(_)) {
            let reference = form.reference.read(cx).value().trim().to_string();
            if !reference.starts_with("op://") {
                self.failed = Some(ProviderError::Reference(reference).to_string());
                cx.notify();
                return;
            }
            Pasted::Reference(reference)
        } else {
            let key = Key::new(&form.key.read(cx).value());
            if key.is_empty() {
                self.failed = Some(ProviderError::NoKey.to_string());
                cx.notify();
                return;
            }
            Pasted::Key(key, form.keep)
        };
        let vault = crate::settings::current(cx).vault;
        form.saving = true;
        self.failed = None;
        let gateway = self.gateway.clone();
        let homes_file = crate::settings::file("key-homes.json");
        let saved = cx.background_spawn(async move {
            let op = find_op();
            let name = job.name().to_string();
            let (source, home) = match from {
                Pasted::Reference(reference) => (
                    KeySource::Reference(reference.clone()),
                    Some(KeyHome::OnePassword { reference }),
                ),
                Pasted::Key(key, KeepKeysIn::Nowhere) => (KeySource::Key(key), None),
                Pasted::Key(key, KeepKeysIn::OnePassword) => {
                    let op = op.as_deref().ok_or(KeyHomeError::NoOp)?;
                    let vault = vault.ok_or_else(|| {
                        anyhow::anyhow!("Choose a 1Password vault in Settings ▸ Keys first.")
                    })?;
                    let reference = save_in_1password(op, &vault, &keychain_service(&name), &key)?;
                    (
                        KeySource::Reference(reference.clone()),
                        Some(KeyHome::OnePassword { reference }),
                    )
                }
                Pasted::Key(key, KeepKeysIn::Keychain) => {
                    let service = keychain_service(&name);
                    save_in_keychain(&service, &key)?;
                    (KeySource::Key(key), Some(KeyHome::Keychain { service }))
                }
            };
            let openshell = OpenShell::find()?;
            match &job {
                Job::Add(new) => {
                    openshell
                        .on(gateway)
                        .add_provider_from(new, &source, op.as_deref())?
                }
                Job::Replace(provider) => match &source {
                    KeySource::Key(key) => openshell.on(gateway).replace_key(provider, key)?,
                    _ => anyhow::bail!("A new key is pasted, then kept where the form says."),
                },
                Job::Copy(provider, to) => openshell.clone().on(to.clone()).copy_provider(
                    &openshell.on(gateway),
                    provider,
                    &source,
                    op.as_deref(),
                )?,
            }
            if let (Some(home), Some(path)) = (home, homes_file) {
                let mut homes = KeyHomes::load(&path);
                homes.homes.insert(name.clone(), home);
                homes.save(&path).map_err(anyhow::Error::msg)?;
            }
            anyhow::Ok(name)
        });
        self._working = Some(cx.spawn_in(window, async move |this, cx| {
            let saved = saved.await;
            let _ = this.update_in(cx, |this, window, cx| this.saved(saved, window, cx));
        }));
        cx.notify();
    }

    /// Signs in to ChatGPT with the Mac's Codex, by device code, into a folder of Playpen's, and waits up to a quarter of
    /// an hour for the login to appear there. Its tokens go to OpenShell and the folder is deleted; the account is
    /// kept, for the sandboxes that use the provider.
    fn sign_in(&mut self, job: Job, window: &mut Window, cx: &mut Context<Self>) {
        // Codex signs in by device code into a folder of Playpen's, so nothing opens but the page the person chooses.
        let started = (|| {
            let codex = find_codex().ok_or(ChatGptError::NoCodex)?;
            let login = Login::start()?;
            anyhow::Ok((login, codex))
        })();
        let (mut login, codex) = match started {
            Ok(login) => login,
            Err(error) => {
                tracing::warn!("couldn't start signing in to ChatGPT: {error:#}");
                self.failed = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };
        if let Some(form) = &mut self.form {
            form.saving = true;
            form.signing_in = true;
        }
        self.failed = None;
        tracing::info!(provider = %job.name(), "signing in to ChatGPT");
        let accounts = crate::settings::file("chatgpt");
        let gateway = self.gateway.clone();
        self._working = Some(cx.spawn_in(window, async move |this, cx| {
            let (back, code) = cx
                .background_spawn(async move {
                    let code = login.start_device(&codex);
                    (login, code)
                })
                .await;
            login = back;
            match code {
                Ok(code) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(form) = &mut this.form {
                            form.device = Some(code);
                        }
                        cx.notify();
                    });
                }
                Err(error) => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.saved(Err(error.into()), window, cx);
                    });
                    return;
                }
            }
            // Codex gives a quarter of an hour to sign in.
            let deadline = Instant::now() + Duration::from_secs(15 * 60);
            let signed_in = loop {
                if let Some(tokens) = login.tokens() {
                    break tokens;
                }
                if Instant::now() > deadline {
                    break Err(ChatGptError::Unreadable(
                        "nobody signed in within a quarter of an hour".to_string(),
                    ));
                }
                cx.background_executor().timer(Duration::from_secs(1)).await;
            };
            // The login's folder holds its tokens, so it goes as soon as they're read.
            drop(login);
            let saved = match signed_in {
                Err(error) => Err(error.into()),
                Ok(tokens) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(form) = &mut this.form {
                            form.signing_in = false;
                        }
                        cx.notify();
                    });
                    cx.background_spawn(async move {
                        let openshell = OpenShell::find()?;
                        match &job {
                            Job::Add(new) => openshell.on(gateway).add_signed_in(new, &tokens)?,
                            Job::Replace(provider) => {
                                openshell.on(gateway).sign_in_again(provider, &tokens)?;
                            }
                            // A login of its own on the other gateway, since each renews its own.
                            Job::Copy(provider, to) => {
                                let new = NewProvider::new(
                                    Kind::ChatGptSubscription,
                                    &provider.name,
                                    "",
                                )?;
                                openshell.on(to.clone()).add_signed_in(&new, &tokens)?;
                            }
                        }
                        if let Some(accounts) = accounts {
                            tokens.account.save(&accounts, job.name())?;
                        }
                        anyhow::Ok(job.name().to_string())
                    })
                    .await
                }
            };
            let _ = this.update_in(cx, |this, window, cx| this.saved(saved, window, cx));
        }));
        cx.notify();
    }

    /// Closes the form once a provider is saved, or says why it wasn't.
    fn saved(
        &mut self,
        saved: anyhow::Result<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match saved {
            Ok(name) => {
                tracing::info!(provider = %name, "saved a provider");
                Toast::new(t!("Saved “%{name}”", name = name).into_owned())
                    .key("provider-saved")
                    .show(window, cx);
                self.form = None;
                self.refresh(window, cx);
            }
            Err(error) => {
                tracing::warn!("couldn't save a provider: {error:#}");
                if let Some(form) = &mut self.form {
                    form.saving = false;
                    form.signing_in = false;
                }
                self.failed = Some(format!("{error:#}"));
            }
        }
        cx.notify();
    }

    /// Deletes the provider once the person says to.
    fn delete(&mut self, provider: Provider, window: &mut Window, cx: &mut Context<Self>) {
        let delete = t!("Delete");
        let answer = window.prompt(
            PromptLevel::Warning,
            &t!("Delete “%{name}”?", name = provider.name),
            Some(&t!(
                "OpenShell forgets its key. Sandboxes using it have to be deleted first."
            )),
            &[&*delete, &*t!("Cancel")],
            cx,
        );
        let gateway = self.gateway.clone();
        self._working = Some(cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let name = provider.name.clone();
            let accounts = crate::settings::file("chatgpt");
            let deleted = cx
                .background_spawn(async move {
                    OpenShell::find()?.on(gateway).delete_provider(&provider)?;
                    if provider.playpen_kind() == Some(Kind::ChatGptSubscription)
                        && let Some(accounts) = accounts
                    {
                        Account::forget(&accounts, &provider.name);
                    }
                    Ok::<_, playpen_core::OpenShellError>(())
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                match deleted {
                    Ok(()) => {
                        tracing::info!(provider = %name, "deleted a provider");
                        Toast::new(t!("Deleted “%{name}”", name = name).into_owned())
                            .key("provider-deleted")
                            .show(window, cx);
                        this.failed = None;
                    }
                    Err(error) => {
                        tracing::warn!(provider = %name, "couldn't delete a provider: {error}");
                        this.failed = Some(error.to_string());
                    }
                }
                this.refresh(window, cx);
                cx.notify();
            });
        }));
    }

    /// Every gateway's providers, each in a recessed card matching the rest of Playpen.
    fn groups(&self, cx: &Context<Self>) -> Div {
        let listings = match &self.listings {
            Ok(listings) => listings,
            Err(error) => {
                return div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone());
            }
        };
        if listings.is_empty() {
            return note(t!("Asking OpenShell…"), cx);
        }
        v_flex()
            .gap_5()
            .children(listings.iter().map(|(gateway, listing)| {
                let here = self.gateway.name() == Some(gateway.as_str());
                let failed = matches!(listing, Listing::Failed(_));
                let adding = gateway.clone();
                let adding_service = gateway.clone();
                let count = match listing {
                    Listing::Listed(providers) => Some(providers.len()),
                    _ => None,
                };
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .gap_2()
                                    .items_center()
                                    .min_w_0()
                                    .child(div().font_semibold().child(gateway.clone()))
                                    .when_some(count.filter(|count| *count > 0), |row, count| {
                                        row.child(shell::count(count, cx))
                                    }),
                            )
                            .child(
                                Button::new(ElementId::from((
                                    ElementId::from("add-provider"),
                                    gateway.clone(),
                                )))
                                .xsmall()
                                .outline()
                                .icon(assets::IconName::Plus)
                                .label(t!("Model Key…").into_owned())
                                .disabled(failed)
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.gateway = Gateway::named(adding.clone());
                                        this.start_adding(window, cx);
                                    },
                                )),
                            )
                            .child(
                                Button::new(ElementId::from((
                                    ElementId::from("add-service"),
                                    gateway.clone(),
                                )))
                                .xsmall()
                                .outline()
                                .icon(assets::IconName::Plus)
                                .label(t!("Service…").into_owned())
                                .disabled(failed)
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.gateway = Gateway::named(adding_service.clone());
                                        this.start_adding_service(window, cx);
                                    },
                                )),
                            ),
                    )
                    .child(self.list(gateway, listing, cx))
                    .when(here, |group| {
                        group.when_some(self.failed.clone(), |group, failed| {
                            group.child(div().text_sm().text_color(cx.theme().danger).child(failed))
                        })
                    })
            }))
    }

    /// One gateway's providers inside a recessed card, with a compact action menu instead of a wall of buttons.
    fn list(&self, gateway: &str, listing: &Listing, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let providers = match listing {
            Listing::Loading => return note(t!("Asking OpenShell…"), cx),
            Listing::Failed(error) => {
                return div()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error.clone());
            }
            Listing::Listed(providers) if providers.is_empty() => {
                return div()
                    .px_3()
                    .py_3()
                    .rounded(theme.radius * 1.5)
                    .border_1()
                    .border_color(theme.border)
                    .bg(shell::recessed_colour(theme))
                    .child(note(
                        t!("None yet. Add a model key or service for sandboxes on this gateway."),
                        cx,
                    ));
            }
            Listing::Listed(providers) => providers,
        };
        let count = providers.len();
        v_flex()
            .rounded(theme.radius * 1.5)
            .border_1()
            .border_color(theme.border)
            .bg(shell::recessed_colour(theme))
            .children(providers.iter().enumerate().map(|(at, provider)| {
                let is_service = provider.kind.starts_with("playpen-service-");
                let kind = match provider.playpen_kind() {
                    Some(kind) => kind.name().to_string(),
                    None if is_service => t!("Service").into_owned(),
                    None => provider.kind.clone(),
                };
                let replace = provider.clone();
                let delete = provider.clone();
                let can_replace = provider.playpen_kind().is_some();
                let id = |what: &'static str| {
                    ElementId::from((
                        ElementId::from(what),
                        format!("{gateway}/{}", provider.name),
                    ))
                };
                let on = gateway.to_string();
                let home_badge = match self.homes.homes.get(&provider.name) {
                    Some(KeyHome::OnePassword { .. }) => Some(t!("1Password").into_owned()),
                    Some(KeyHome::Keychain { .. }) => Some(t!("Keychain").into_owned()),
                    None => None,
                };
                let others: Vec<String> = self
                    .gateways
                    .iter()
                    .filter(|other| other.as_str() != gateway)
                    .cloned()
                    .collect();
                let view = cx.entity().downgrade();
                let copying = provider.clone();
                let from_gw = on.clone();
                let delete_on = on.clone();
                let more = Button::new(id("provider-more"))
                    .xsmall()
                    .ghost()
                    .icon(assets::IconName::Ellipsis)
                    .tooltip(t!("More Actions").into_owned())
                    .dropdown_menu(move |mut menu, _, _| {
                        for target in &others {
                            let view = view.clone();
                            let provider = copying.clone();
                            let from = from_gw.clone();
                            let to = Gateway::named(target.clone());
                            menu = menu.item(
                                PopupMenuItem::new(
                                    t!("Copy to %{gateway}", gateway = target).into_owned(),
                                )
                                .icon(assets::IconName::Copy)
                                .on_click(move |_, window, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        this.gateway = Gateway::named(from.clone());
                                        this.add_to_gateway(
                                            provider.clone(),
                                            to.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                            );
                        }
                        if !others.is_empty() {
                            menu = menu.separator();
                        }
                        let view = view.clone();
                        let delete = delete.clone();
                        let on = delete_on.clone();
                        menu.item(
                            PopupMenuItem::new(t!("Delete…").into_owned())
                                .icon(assets::IconName::Trash)
                                .on_click(move |_, window, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        this.gateway = Gateway::named(on.clone());
                                        this.delete(delete.clone(), window, cx);
                                    });
                                }),
                        )
                    });
                h_flex()
                    .gap_2p5()
                    .px_3()
                    .py_2()
                    .items_center()
                    .when(at + 1 < count, |row| {
                        row.border_b_1().border_color(shell::hairline(theme))
                    })
                    .child(
                        Icon::new(if is_service {
                            assets::IconName::Globe
                        } else {
                            assets::IconName::KeyRound
                        })
                        .small()
                        .flex_none()
                        .text_color(muted),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_sm()
                                            .font_medium()
                                            .child(provider.name.clone()),
                                    )
                                    .when_some(home_badge, |row, badge| {
                                        row.child(
                                            shell::badge(Tag::secondary()).xsmall().child(badge),
                                        )
                                    }),
                            )
                            .child(div().text_xs().text_color(muted).child(kind)),
                    )
                    .when(can_replace, |row| {
                        row.child(
                            Button::new(id("replace-key"))
                                .xsmall()
                                .outline()
                                .label(
                                    if provider.playpen_kind() == Some(Kind::ChatGptSubscription) {
                                        t!("Sign In Again…")
                                    } else {
                                        t!("Replace Key…")
                                    }
                                    .into_owned(),
                                )
                                .on_click(cx.listener({
                                    let on = on.clone();
                                    move |this, _, window, cx| {
                                        this.gateway = Gateway::named(on.clone());
                                        this.start_replacing(replace.clone(), window, cx);
                                    }
                                })),
                        )
                    })
                    .child(more)
            }))
    }

    /// Where the key comes from, pasted or a 1Password reference, and where a pasted one is kept besides OpenShell.
    fn key_fields(&self, form: &Form, cx: &Context<Self>) -> Div {
        let has_op = find_op().is_some();
        let has_vault = crate::settings::current(cx).vault.is_some();
        let replacing = form.replacing.is_some();
        let from = |reference: bool, label: std::borrow::Cow<str>, id: &'static str| {
            Radio::new(id)
                .label(label.into_owned())
                .checked(form.from_1password == reference)
                .disabled(form.saving)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = &mut this.form {
                        form.from_1password = reference;
                        cx.notify();
                    }
                }))
        };
        let keep = |choice: KeepKeysIn, label: std::borrow::Cow<str>, id: &'static str| {
            Radio::new(id)
                .label(label.into_owned())
                .checked(form.keep == choice)
                .disabled(
                    form.saving || (choice == KeepKeysIn::OnePassword && !(has_op && has_vault)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = &mut this.form {
                        form.keep = choice;
                        cx.notify();
                    }
                }))
        };
        let reference = form.from_1password && !replacing;
        v_flex()
            .gap_3()
            // A reference only for a new key: replacing one pastes it.
            .when(has_op && !replacing, |fields| {
                fields.child(field(
                    t!("Key from"),
                    h_flex()
                        .gap_4()
                        .child(from(false, t!("Paste"), "key-pasted"))
                        .child(from(true, t!("1Password"), "key-1password")),
                    cx,
                ))
            })
            .child(if reference {
                field(
                    t!("1Password reference"),
                    v_flex()
                        .gap_1()
                        .child(div().w(rems(20.)).child(small_field(&form.reference)))
                        .child(note(
                            t!("In 1Password, open a field's menu and choose Copy Secret Reference. The 1Password CLI hands the key to OpenShell and reuses it for other gateways."),
                            cx,
                        )),
                    cx,
                )
            } else {
                field(
                    t!("Key"),
                    v_flex()
                        .gap_1()
                        .child(div().w(rems(20.)).child(small_field(&form.key)))
                        .when(form.copying.is_none(), |this| {
                            this.child(note(t!(form.kind.where_to_get_a_key()), cx))
                        })
                        .when(form.kind == Kind::ClaudeSubscription, |this| {
                            this.child(self.claude_sign_in(form, cx))
                        }),
                    cx,
                )
            })
            .when(!reference, |fields| {
                fields.child(field(
                    t!("Keep it in"),
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_4()
                                .child(keep(KeepKeysIn::Nowhere, t!("OpenShell only"), "keep-only"))
                                .child(keep(KeepKeysIn::OnePassword, t!("1Password too"), "keep-op"))
                                .child(keep(KeepKeysIn::Keychain, t!("the Keychain too"), "keep-kc")),
                        )
                        .child(note(
                            if has_op && !has_vault {
                                t!("Kept there too, it can go to another gateway, or be replaced everywhere, without being pasted again. For 1Password, choose a vault in Settings ▸ Keys.")
                            } else {
                                t!("Kept there too, it can go to another gateway, or be replaced everywhere, without being pasted again.")
                            },
                            cx,
                        )),
                    cx,
                ))
            })
    }

    /// Runs `claude setup-token` for the person, shows the page to sign in at, and once they have, fills in the token
    /// and saves the provider as if it had been pasted, kept where the form says.
    fn sign_in_with_claude(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        if form.claude.is_some() || form.saving {
            return;
        }
        let started = find_claude()
            .ok_or(playpen_core::ClaudeTokenError::NoClaude)
            .and_then(|claude| TokenSignIn::start(&claude));
        let session = match started {
            Ok(session) => Arc::new(Mutex::new(session)),
            Err(error) => {
                tracing::warn!("couldn't start signing in with Claude: {error}");
                self.failed = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        tracing::info!("signing in with Claude for a token");
        self.failed = None;
        let code = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("Only if the page shows one").into_owned())
        });
        let watching = Arc::clone(&session);
        let waiting = cx.spawn_in(window, async move |this, cx| {
            // Claude Code's sign-in lasts a while; a quarter of an hour is plenty.
            let deadline = Instant::now() + Duration::from_secs(15 * 60);
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let (link, token, ended) = match watching.lock() {
                    Ok(mut session) => (session.link(), session.token(), session.ended()),
                    Err(_) => break,
                };
                let timed_out = Instant::now() > deadline;
                let done = this.update_in(cx, |this, window, cx| {
                    let Some(form) = &mut this.form else {
                        return true;
                    };
                    let Some(claude) = &mut form.claude else {
                        return true;
                    };
                    if claude.link != link {
                        claude.link = link;
                        cx.notify();
                    }
                    if let Some(token) = token {
                        tracing::info!("Claude gave a token");
                        let key = token.reveal().to_string();
                        form.key
                            .update(cx, |field, cx| field.set_value(key, window, cx));
                        form.from_1password = false;
                        form.from_claude = true;
                        form.claude = None;
                        this.save(window, cx);
                        return true;
                    }
                    let failed = ended.map(|error| error.to_string()).or_else(|| {
                        timed_out.then(|| {
                            t!("Nobody signed in within a quarter of an hour.").into_owned()
                        })
                    });
                    if let Some(failed) = failed {
                        tracing::warn!("signing in with Claude ended: {failed}");
                        form.claude = None;
                        this.failed = Some(failed);
                        cx.notify();
                        return true;
                    }
                    false
                });
                if done.unwrap_or(true) {
                    break;
                }
            }
        });
        form.claude = Some(ClaudeSignIn {
            session,
            link: None,
            code,
            sent: false,
            _waiting: waiting,
        });
        cx.notify();
    }

    /// Gives Claude Code the code the sign-in page showed.
    fn send_claude_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(claude) = self.form.as_ref().and_then(|form| form.claude.as_ref()) else {
            return;
        };
        let code = claude.code.read(cx).value().trim().to_string();
        if code.is_empty() {
            return;
        }
        let sent = claude
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("the sign-in stopped"))
            .and_then(|mut session| session.send_code(&code).map_err(anyhow::Error::from));
        claude
            .code
            .update(cx, |field, cx| field.set_value("", window, cx));
        let sent = sent.map_err(|error| format!("{error:#}"));
        if let Some(claude) = self.form.as_mut().and_then(|form| form.claude.as_mut()) {
            claude.sent = sent.is_ok();
        }
        self.failed = sent.err();
        cx.notify();
    }

    /// The Sign In with Claude button, or, while it's signing in, the page to open and a box for a code.
    fn claude_sign_in(&self, form: &Form, cx: &Context<Self>) -> Div {
        let Some(claude) = &form.claude else {
            if form.from_claude {
                return h_flex()
                    .gap_2()
                    .child(
                        Icon::new(assets::IconName::Check)
                            .size_4()
                            .text_color(cx.theme().success),
                    )
                    .child(note(
                        t!("Claude gave a token, and it's in the Key field."),
                        cx,
                    ));
            }
            return h_flex().child(
                Button::new("sign-in-with-claude")
                    .small()
                    .icon(assets::IconName::KeyRound)
                    .label(t!("Sign In with Claude…").into_owned())
                    .disabled(form.saving)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.sign_in_with_claude(window, cx)),
                    ),
            );
        };
        let link = claude.link.clone();
        v_flex()
            .gap_2()
            .child(note(
                match (&link, claude.sent) {
                    (_, true) => t!("Code sent. Waiting for Claude Code to swap it for a token…"),
                    (Some(_), false) => t!("Open the sign-in page and sign in to Claude. Playpen fills in the token and adds the provider once you have."),
                    (None, false) => t!("Asking Claude Code for a page to sign in at…"),
                },
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("open-claude-sign-in")
                            .primary()
                            .small()
                            .label(t!("Open Sign-In Page").into_owned())
                            .disabled(link.is_none())
                            .on_click({
                                let link = link.clone();
                                move |_, _, cx| {
                                    if let Some(link) = &link {
                                        cx.open_url(link);
                                    }
                                }
                            }),
                    )
                    .child(
                        Button::new("copy-claude-sign-in")
                            .small()
                            .label(t!("Copy Link").into_owned())
                            .disabled(link.is_none())
                            .on_click(move |_, window, cx| {
                                if let Some(link) = &link {
                                    cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                                    Toast::new(t!("Copied sign-in link").into_owned())
                                        .key("copy-link")
                                        .show(window, cx);
                                }
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(rems(16.)).child(small_field(&claude.code)))
                    .child(
                        Button::new("send-claude-code")
                            .small()
                            .loading(claude.sent)
                            .disabled(claude.sent)
                            .label(t!("Send Code").into_owned())
                            .on_click(cx.listener(|this, _, window, cx| this.send_claude_code(window, cx))),
                    ),
            )
            .child(note(
                t!("If the page shows a code instead, paste it here. Cancel stops the sign-in."),
                cx,
            ))
    }

    /// Puts `provider` on the gateway `to`: from where its key lives when Playpen knows, or by asking for the key,
    /// or, for a ChatGPT login, by signing in again there.
    fn add_to_gateway(
        &mut self,
        provider: Provider,
        to: Gateway,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let home = self.homes.homes.get(&provider.name).cloned();
        let chatgpt = provider.playpen_kind() == Some(Kind::ChatGptSubscription);
        if chatgpt || home.is_none() {
            let kind = provider.playpen_kind().unwrap_or(Kind::OpenRouter);
            self.open_form(None, kind, window, cx);
            if let Some(form) = &mut self.form {
                form.copying = Some((provider, to));
            }
            cx.notify();
            return;
        }
        let Some(home) = home else {
            return;
        };
        let from = self.gateway.clone();
        self.failed = None;
        let name = provider.name.clone();
        let copied = cx.background_spawn(async move {
            let openshell = OpenShell::find()?;
            let source = home.source()?;
            openshell.clone().on(to).copy_provider(
                &openshell.on(from),
                &provider,
                &source,
                find_op().as_deref(),
            )?;
            anyhow::Ok(())
        });
        self._working = Some(cx.spawn_in(window, async move |this, cx| {
            let copied = copied.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match copied {
                    Ok(()) => tracing::info!(provider = %name, "added a provider to another gateway"),
                    Err(error) => {
                        tracing::warn!(provider = %name, "couldn't add a provider to another gateway: {error:#}");
                        this.failed = Some(format!("{error:#}"));
                    }
                }
                this.refresh(window, cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn form(&self, form: &Form, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let adding = form.replacing.is_none() && form.copying.is_none();
        let title = match (&form.copying, &form.replacing) {
            (Some((provider, to)), _) => t!(
                "Add “%{name}” to %{gateway}",
                name = provider.name,
                gateway = to.name().unwrap_or("active")
            ),
            (None, Some(provider)) if form.kind.signs_in() => {
                t!("Sign “%{name}” In Again", name = provider.name)
            }
            (None, Some(provider)) => t!("New Key for “%{name}”", name = provider.name),
            (None, None) => match self.gateway.name() {
                Some(gw) => t!("Add a Model Key · %{gateway}", gateway = gw),
                None => t!("Add a Model Key"),
            },
        };
        let kinds = div().w(rems(16.)).child(
            Select::new(&form.kinds)
                .small()
                .search_placeholder(t!("Search").into_owned())
                .disabled(form.saving),
        );
        let header = h_flex()
            .flex_none()
            .justify_between()
            .items_center()
            .gap_2()
            .px_4()
            .h(rems(38. / 13.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .min_w_0()
                    .child(
                        Button::new("back-from-provider")
                            .ghost()
                            .xsmall()
                            .disabled(form.saving && !form.signing_in)
                            .icon(assets::IconName::ArrowLeft)
                            .tooltip(t!("Back to Providers").into_owned())
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                    )
                    .child(div().truncate().font_semibold().child(title.into_owned())),
            );
        let body = v_flex()
            .gap_4()
            .p_4()
            .when(adding, |this| {
                this.child(
                    h_flex()
                        .flex_wrap()
                        .gap_4()
                        .items_start()
                        .child(field(t!("Kind"), kinds, cx))
                        .child(field(
                            t!("Name"),
                            div().w(rems(16.)).child(small_field(&form.name)),
                            cx,
                        )),
                )
            })
            .when(adding && form.kind.needs_address(), |this| {
                this.child(field(
                    t!("Address"),
                    v_flex()
                        .gap_1()
                        .child(div().w(rems(20.)).child(small_field(&form.address)))
                        .child(note(
                            t!("The proxy's address, as the agents would reach it."),
                            cx,
                        )),
                    cx,
                ))
            })
            .map(|this| {
                if form.kind.signs_in() {
                    match (&form.device, form.signing_in) {
                        (Some(device), true) => this.child(device_code(device, cx)),
                        (None, true) => {
                            this.child(note(t!("Asking Codex for a code to sign in with…"), cx))
                        }
                        _ => this.child(note(t!(form.kind.where_to_get_a_key()), cx)),
                    }
                } else {
                    this.child(self.key_fields(form, cx))
                }
            });
        let footer = h_flex()
            .flex_none()
            .justify_between()
            .items_center()
            .gap_3()
            .px_4()
            .py_2p5()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when_some(self.failed.clone(), |slot, failed| {
                        slot.child(div().text_sm().text_color(theme.danger).child(failed))
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(
                        Button::new("cancel-provider")
                            .ghost()
                            .small()
                            .disabled(form.saving && !form.signing_in)
                            .label(t!("Cancel").into_owned())
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                    )
                    .child(
                        Button::new("save-provider")
                            .primary()
                            .small()
                            .loading(form.saving)
                            .label(
                                match (form.kind.signs_in(), adding) {
                                    (true, _) => t!("Sign In…"),
                                    (false, true) => t!("Add Model Key"),
                                    (false, false) => t!("Replace Key"),
                                }
                                .into_owned(),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            );
        v_flex()
            .size_full()
            .key_context(FORM_CONTEXT)
            .on_action(cx.listener(|this, _: &DismissForm, _, cx| {
                this.dismiss_form(cx);
            }))
            .child(header)
            .child(
                div()
                    .id("provider-form-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(footer)
            .into_any_element()
    }
}

/// What saving does.
enum Job {
    Add(NewProvider),
    Replace(Provider),
    /// Putting a provider on another gateway.
    Copy(Provider, Gateway),
}

/// A key as the form gives it: pasted, with where to keep it besides OpenShell, or a 1Password reference.
enum Pasted {
    Key(Key, KeepKeysIn),
    Reference(String),
}

impl Job {
    fn name(&self) -> &str {
        match self {
            Job::Add(new) => &new.name,
            Job::Replace(provider) | Job::Copy(provider, _) => &provider.name,
        }
    }
}

impl Render for Providers {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(form) = &self.form {
            return self.form(form, cx);
        }
        if let Some(service) = &self.service {
            return self.service_form(service, cx);
        }
        div()
            .id("providers")
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p_5()
                    .gap_5()
                    .child(note(
                        t!(
                            "OpenShell holds each key on its gateway. Adding a service here does not grant access to any sandbox until you attach it."
                        ),
                        cx,
                    ))
                    .child(self.groups(cx)),
            )
            .into_any_element()
    }
}

/// The page to sign in at and the one-time code to give it, with buttons to open the one and copy the other.
fn device_code(device: &DeviceCode, cx: &App) -> Div {
    let theme = cx.theme();
    let url = device.url.clone();
    let code = device.code.clone();
    v_flex()
        .gap_2()
        .child(note(
            t!("Open the sign-in page, sign in to ChatGPT, and give it this code. Playpen carries on once you have."),
            cx,
        ))
        .child(
            h_flex()
                .gap_3()
                .child(
                    div()
                        .px_3()
                        .py_1p5()
                        .rounded(theme.radius)
                        .bg(theme.muted)
                        .font_family(theme.mono_font_family.clone())
                        .text_lg()
                        .font_semibold()
                        .child(device.code.clone()),
                )
                .child(
                    Button::new("copy-device-code")
                        .small()
                        .label(t!("Copy Code").into_owned())
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                            Toast::new(t!("Copied sign-in code").into_owned())
                                .key("copy-code")
                                .show(window, cx);
                        }),
                )
                .child(
                    Button::new("open-device-page")
                        .primary()
                        .small()
                        .label(t!("Open Sign-In Page").into_owned())
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                ),
        )
        .child(note(t!("The code lasts a quarter of an hour."), cx))
}

/// A labelled row of the form.
fn field(label: std::borrow::Cow<str>, body: impl IntoElement, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(label.into_owned()),
        )
        .child(body)
}

fn note(text: std::borrow::Cow<str>, cx: &App) -> Div {
    div()
        .max_w(rems(42.))
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into_owned())
}

/// A one-line field at the kit's small size. The kit's Input fills whatever height it's offered, so it's given one.
fn small_field(state: &Entity<InputState>) -> Input {
    Input::new(state).small().h(rems(24. / 13.))
}
