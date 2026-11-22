//! Providers, a place in the main window: the model keys OpenShell holds for sandboxes, added, given new keys and deleted here. Every
//! call to OpenShell runs off the window's thread. A key goes from its field to OpenShell and nowhere else: it isn't
//! kept, logged or shown again.

use gpui_kit::component::IndexPath;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::{Duration, Instant};

use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::radio::Radio;
use playpen_core::{
    Access, Account, ChatGptError, DeviceCode, Gateway, Key, KeySource, Kind, Login, NewProvider,
    NewService, OpenShell, Provider, ProviderError, find_codex, find_op,
};
use playpen_core::{
    KeepKeysIn, KeyHome, KeyHomeError, KeyHomes, keychain_service, save_in_1password,
    save_in_keychain,
};
use rust_i18n::t;

/// The gateway the main window shows, which a provider is for: OpenShell keeps each in the gateway that holds it, so
/// Settings acts on the one whose sandboxes are in front of the person.
#[derive(Debug, Clone, Default)]
pub struct ShownGateway(pub Gateway);

impl Global for ShownGateway {}

/// The providers part of Settings.
pub struct Providers {
    /// The gateway it lists and acts on, taken from the main window each time it lists.
    gateway: Gateway,
    listing: Listing,
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
    _chose: Subscription,
}

/// Adding a service: a key for something other than a model, sent to the one host given.
struct ServiceForm {
    name: Entity<InputState>,
    address: Entity<InputState>,
    variable: Entity<InputState>,
    access: Access,
    from: From,
    key: Entity<InputState>,
    reference: Entity<InputState>,
    environment: Entity<InputState>,
    keychain: Entity<InputState>,
    saving: bool,
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
            listing: Listing::Loading,
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
            self.listing = Listing::Listed(vec![
                stand_in("claude-subscription", "claude-code-oauth"),
                stand_in("openrouter", "playpen-openrouter"),
                stand_in("work", "playpen-litellm-work"),
                stand_in("github", "playpen-service-github"),
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
        let gateway = self.gateway.clone();
        if let Some(path) = crate::settings::file("key-homes.json") {
            self.homes = KeyHomes::load(&path);
        }
        self._working = Some(cx.spawn_in(window, async move |this, cx| {
            let (listed, gateways) = cx
                .background_spawn(async move {
                    let openshell = OpenShell::find();
                    let gateways = openshell
                        .as_ref()
                        .ok()
                        .and_then(|openshell| openshell.gateways().ok())
                        .unwrap_or_default();
                    (
                        openshell.and_then(|openshell| openshell.on(gateway).providers()),
                        gateways,
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.gateways = gateways.into_iter().map(|gateway| gateway.name).collect();
                this.listing = match listed {
                    Ok(providers) => Listing::Listed(providers),
                    Err(error) => {
                        tracing::warn!("couldn't list the providers: {error}");
                        Listing::Failed(error.to_string())
                    }
                };
                cx.notify();
            });
        }));
    }

    /// Opens the form for a new service.
    pub(crate) fn start_adding_service(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = |placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            let placeholder = placeholder.to_string();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let name = field("github", window, cx);
        name.read(cx).focus_handle(cx).focus(window, cx);
        self.form = None;
        self.failed = None;
        self.service = Some(ServiceForm {
            name,
            address: field("api.github.com", window, cx),
            variable: field("GITHUB_TOKEN", window, cx),
            access: Access::ReadOnly,
            from: if find_op().is_some() {
                From::Reference
            } else {
                From::Pasted
            },
            key: cx.new(|cx| InputState::new(window, cx).masked(true)),
            reference: field("op://Private/GitHub/token", window, cx),
            environment: field("Environment ID", window, cx),
            keychain: field("Keychain item", window, cx),
            saving: false,
        });
        cx.notify();
    }

    /// Adds the service, off the window's thread. A 1Password key goes from `op` to OpenShell, never through here.
    fn save_service(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.service else {
            return;
        };
        if form.saving {
            return;
        }
        let new = match NewService::new(
            &form.name.read(cx).value(),
            &form.address.read(cx).value(),
            &form.variable.read(cx).value(),
            form.access,
        ) {
            Ok(new) => new,
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
            From::Keychain => KeySource::Keychain(form.keychain.read(cx).value().to_string()),
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
                        this.service = None;
                        this.refresh(window, cx);
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

    fn service_form(&self, form: &ServiceForm, cx: &Context<Self>) -> Div {
        let has_op = find_op().is_some();
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
                    "In 1Password, a field's menu has Copy Secret Reference. `op` reads it and hands it to OpenShell."
                ),
            ),
            From::Keychain => (
                &form.keychain,
                t!(
                    "The item's name, as Keychain Access shows it under Where. macOS asks once whether `security` may read it."
                ),
            ),
            From::Environment => (
                &form.environment,
                t!(
                    "From the Environment's Manage environment ▸ Copy environment ID. It needs a secret named as the variable."
                ),
            ),
        };
        v_flex()
            .gap_3()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(div().font_semibold().child(t!("Add a Service").into_owned()))
            .child(field(t!("Name"), div().w(rems(16.)).child(small_field(&form.name)), cx))
            .child(field(
                t!("Host"),
                v_flex()
                    .gap_1()
                    .child(div().w(rems(20.)).child(small_field(&form.address)))
                    .child(note(t!("The only host its key is sent to."), cx)),
                cx,
            ))
            .child(field(
                t!("Variable"),
                v_flex()
                    .gap_1()
                    .child(div().w(rems(16.)).child(small_field(&form.variable)))
                    .child(note(
                        t!("Where programs in the sandbox find its stand-in, sent as a bearer token."),
                        cx,
                    )),
                cx,
            ))
            .child(field(
                t!("Access"),
                h_flex()
                    .gap_4()
                    .child(access(Access::ReadOnly, t!("Read only"), "access-read"))
                    .child(access(Access::ReadWrite, t!("Read and write"), "access-write")),
                cx,
            ))
            .child(field(
                t!("Key from"),
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_4()
                            .child(from(From::Reference, t!("1Password reference"), "from-reference"))
                            .child(from(From::Environment, t!("1Password Environment"), "from-environment"))
                            .child(from(From::Keychain, t!("Keychain"), "from-keychain"))
                            .child(from(From::Pasted, t!("Paste"), "from-pasted")),
                    )
                    .child(div().w(rems(20.)).child(small_field(key_field)))
                    .child(note(key_note, cx))
                    .when(!has_op, |this| {
                        this.child(note(
                            t!("Install 1Password's command line, `brew install 1password-cli`, to take keys from 1Password."),
                            cx,
                        ))
                    }),
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("cancel-service")
                            .ghost()
                            .small()
                            .disabled(form.saving)
                            .label(t!("Cancel").into_owned())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.service = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("save-service")
                            .primary()
                            .small()
                            .loading(form.saving)
                            .label(t!("Add Service").into_owned())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_service(window, cx);
                            })),
                    ),
            )
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
        let keep = match crate::settings::current(cx).keep_keys_in {
            KeepKeysIn::OnePassword if find_op().is_none() => KeepKeysIn::Nowhere,
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

    fn list(&self, cx: &Context<Self>) -> Div {
        let muted = cx.theme().muted_foreground;
        let providers = match &self.listing {
            Listing::Loading => return note(t!("Asking OpenShell…"), cx),
            Listing::Failed(error) => {
                return div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone());
            }
            Listing::Listed(providers) if providers.is_empty() => {
                return note(
                    t!("None yet. Add one for each model service whose key your agents use."),
                    cx,
                );
            }
            Listing::Listed(providers) => providers,
        };
        v_flex().gap_2().children(providers.iter().map(|provider| {
            let kind = match provider.playpen_kind() {
                Some(kind) => kind.name().to_string(),
                None if provider.kind.starts_with("playpen-service-") => t!("Service").into_owned(),
                None => provider.kind.clone(),
            };
            let replace = provider.clone();
            let delete = provider.clone();
            let id = |what: &'static str| {
                ElementId::from((ElementId::from(what), provider.name.clone()))
            };
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().child(provider.name.clone()))
                        .child(div().text_sm().text_color(muted).child(
                            match self.homes.homes.get(&provider.name) {
                                Some(KeyHome::OnePassword { .. }) => {
                                    t!("%{kind} · key in 1Password", kind = kind).into_owned()
                                }
                                Some(KeyHome::Keychain { .. }) => {
                                    t!("%{kind} · key in the Keychain", kind = kind).into_owned()
                                }
                                None => kind,
                            },
                        )),
                )
                .child(
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
                        .disabled(provider.playpen_kind().is_none() || self.form.is_some())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_replacing(replace.clone(), window, cx);
                        })),
                )
                .child({
                    let others: Vec<String> = self
                        .gateways
                        .iter()
                        .filter(|gateway| Some(gateway.as_str()) != self.gateway.name())
                        .cloned()
                        .collect();
                    let view = cx.entity().downgrade();
                    let copying = provider.clone();
                    Button::new(id("add-to-gateway"))
                        .xsmall()
                        .outline()
                        .label(t!("Add to Gateway…").into_owned())
                        .disabled(self.form.is_some() || others.is_empty())
                        .dropdown_menu(move |mut menu, _, _| {
                            for gateway in &others {
                                let view = view.clone();
                                let provider = copying.clone();
                                let to = Gateway::named(gateway.clone());
                                menu = menu.item(PopupMenuItem::new(gateway.clone()).on_click(
                                    move |_, window, cx| {
                                        let _ = view.update(cx, |this, cx| {
                                            this.add_to_gateway(
                                                provider.clone(),
                                                to.clone(),
                                                window,
                                                cx,
                                            );
                                        });
                                    },
                                ));
                            }
                            menu
                        })
                })
                .child(
                    Button::new(id("delete-provider"))
                        .xsmall()
                        .outline()
                        .label(t!("Delete…").into_owned())
                        .disabled(self.form.is_some())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.delete(delete.clone(), window, cx);
                        })),
                )
        }))
    }

    /// Where the key comes from, pasted or a 1Password reference, and where a pasted one is kept besides OpenShell.
    fn key_fields(&self, form: &Form, cx: &Context<Self>) -> Div {
        let has_op = find_op().is_some();
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
                .disabled(form.saving || (choice == KeepKeysIn::OnePassword && !has_op))
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
                            t!("In 1Password, a field's menu has Copy Secret Reference. `op` gives the key to OpenShell, and it's taken from there for other gateways too."),
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
                            t!("Kept there too, it can go to another gateway, or be replaced everywhere, without being pasted again."),
                            cx,
                        )),
                    cx,
                ))
            })
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

    fn form(&self, form: &Form, cx: &Context<Self>) -> Div {
        let adding = form.replacing.is_none() && form.copying.is_none();
        let title = match (&form.copying, &form.replacing) {
            (Some((provider, to)), _) => t!(
                "Add “%{name}” to the %{gateway} gateway",
                name = provider.name,
                gateway = to.name().unwrap_or("active")
            ),
            (None, Some(provider)) if form.kind.signs_in() => {
                t!("Sign “%{name}” In Again", name = provider.name)
            }
            (None, Some(provider)) => t!("New Key for “%{name}”", name = provider.name),
            (None, None) => t!("Add a Provider"),
        };
        let kinds = div().w(rems(16.)).child(
            Select::new(&form.kinds)
                .small()
                .search_placeholder(t!("Search").into_owned())
                .disabled(form.saving),
        );
        v_flex()
            .gap_3()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(div().font_semibold().child(title.into_owned()))
            .when(adding, |this| {
                this.child(field(t!("Kind"), kinds, cx)).child(field(
                    t!("Name"),
                    div().w(rems(16.)).child(small_field(&form.name)),
                    cx,
                ))
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
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
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
                                    (false, true) => t!("Add"),
                                    (false, false) => t!("Replace Key"),
                                }
                                .into_owned(),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
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
        v_flex()
            .p_4()
            .gap_4()
            .when_some(self.gateway.name().map(str::to_string), |this, gateway| {
                this.child(note(
                    t!("These are the providers on the “%{gateway}” gateway.", gateway = gateway),
                    cx,
                ))
            })
            .child(note(
                t!(
                    "OpenShell holds each key. A sandbox's agent sees only a stand-in, which OpenShell swaps for the key on requests to that service, so one provider serves every sandbox you attach it to."
                ),
                cx,
            ))
            .child(self.list(cx))
            .when_some(self.failed.clone(), |this, failed| {
                this.child(div().text_sm().text_color(cx.theme().danger).child(failed))
            })
            .map(|this| match (&self.form, &self.service) {
                (Some(form), _) => this.child(self.form(form, cx)),
                (None, Some(service)) => this.child(self.service_form(service, cx)),
                (None, None) => this.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("add-provider")
                                .outline()
                                .label(t!("Add Model Key…").into_owned())
                                .disabled(matches!(self.listing, Listing::Failed(_)))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.start_adding(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("add-service")
                                .outline()
                                .label(t!("Add Service…").into_owned())
                                .disabled(matches!(self.listing, Listing::Failed(_)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.start_adding_service(window, cx);
                                })),
                        ),
                ),
            })
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
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
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
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into_owned())
}

/// A one-line field at the kit's small size. The kit's Input fills whatever height it's offered, so it's given one.
fn small_field(state: &Entity<InputState>) -> Input {
    Input::new(state).small().h(rems(24. / 13.))
}
