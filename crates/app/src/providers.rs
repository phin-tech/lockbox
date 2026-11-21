//! Settings ▸ Providers: the model keys OpenShell holds for sandboxes, added, given new keys and deleted here. Every
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

use playpen_core::{
    Account, ChatGptError, Key, Kind, Login, NewProvider, OpenShell, Provider, ProviderError,
    find_codex,
};
use rust_i18n::t;

/// The providers part of Settings.
pub struct Providers {
    listing: Listing,
    form: Option<Form>,
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
    /// Whether it's waiting for the person to sign in, in Terminal.
    signing_in: bool,
    _chose: Subscription,
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
            listing: Listing::Loading,
            form: None,
            failed: None,
            _working: None,
        };
        this.refresh(window, cx);
        this
    }

    /// Asks OpenShell for its providers again.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            ]);
            if let Ok(kind) = std::env::var("PLAYPEN_SHOT_ADD") {
                let kind = Kind::ALL
                    .into_iter()
                    .find(|known| known.suggested_name() == kind)
                    .unwrap_or(Kind::OpenRouter);
                self.open_form(None, kind, window, cx);
            }
            return;
        }
        self._working = Some(cx.spawn_in(window, async move |this, cx| {
            let listed = cx
                .background_spawn(async move {
                    OpenShell::find().and_then(|openshell| openshell.providers())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
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

    fn start_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_form(None, Kind::OpenRouter, window, cx);
    }

    fn start_replacing(&mut self, provider: Provider, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = provider.playpen_kind() else {
            return;
        };
        self.open_form(Some(provider), kind, window, cx);
    }

    fn open_form(
        &mut self,
        replacing: Option<Provider>,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let suggested = kind.suggested_name().to_string();
        let name = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(suggested.clone(), window, cx);
            state
        });
        let address =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://litellm.example.com"));
        let key = cx.new(|cx| InputState::new(window, cx).masked(true));
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
        let job = match &form.replacing {
            Some(provider) => Job::Replace(provider.clone()),
            None => match NewProvider::new(
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
        let key = Key::new(&form.key.read(cx).value());
        if key.is_empty() {
            self.failed = Some(ProviderError::NoKey.to_string());
            cx.notify();
            return;
        }
        form.saving = true;
        self.failed = None;
        let saved = cx.background_spawn(async move {
            let openshell = OpenShell::find()?;
            match &job {
                Job::Add(new) => openshell.add_provider(new, &key)?,
                Job::Replace(provider) => openshell.replace_key(provider, &key)?,
            }
            anyhow::Ok(job.name().to_string())
        });
        self._working = Some(cx.spawn_in(window, async move |this, cx| {
            let saved = saved.await;
            let _ = this.update_in(cx, |this, window, cx| this.saved(saved, window, cx));
        }));
        cx.notify();
    }

    /// Signs in to ChatGPT with the Mac's Codex, in Terminal, into a folder of Playpen's, and waits up to a quarter of
    /// an hour for the login to appear there. Its tokens go to OpenShell and the folder is deleted; the account is
    /// kept, for the sandboxes that use the provider.
    fn sign_in(&mut self, job: Job, window: &mut Window, cx: &mut Context<Self>) {
        let started = (|| {
            let codex = find_codex().ok_or(ChatGptError::NoCodex)?;
            let login = Login::start()?;
            playpen_core::session::show_in_terminal(
                "chatgpt-sign-in",
                &login.command_line(&codex),
            )?;
            anyhow::Ok(login)
        })();
        let login = match started {
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
        self._working = Some(cx.spawn_in(window, async move |this, cx| {
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
                            Job::Add(new) => openshell.add_signed_in(new, &tokens)?,
                            Job::Replace(provider) => openshell.sign_in_again(provider, &tokens)?,
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
        self._working = Some(cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let name = provider.name.clone();
            let accounts = crate::settings::file("chatgpt");
            let deleted = cx
                .background_spawn(async move {
                    OpenShell::find()?.delete_provider(&provider)?;
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
            let kind = provider
                .playpen_kind()
                .map_or_else(|| provider.kind.clone(), |kind| kind.name().to_string());
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
                        .child(div().text_sm().text_color(muted).child(kind)),
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

    fn form(&self, form: &Form, cx: &Context<Self>) -> Div {
        let adding = form.replacing.is_none();
        let title = match &form.replacing {
            Some(provider) if form.kind.signs_in() => {
                t!("Sign “%{name}” In Again", name = provider.name)
            }
            Some(provider) => t!("New Key for “%{name}”", name = provider.name),
            None => t!("Add a Provider"),
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
                    this.child(note(
                        if form.signing_in {
                            t!("Waiting for you to sign in, in Terminal…")
                        } else {
                            t!(form.kind.where_to_get_a_key())
                        },
                        cx,
                    ))
                } else {
                    this.child(field(
                        t!("Key"),
                        v_flex()
                            .gap_1()
                            .child(div().w(rems(20.)).child(small_field(&form.key)))
                            .child(note(t!(form.kind.where_to_get_a_key()), cx)),
                        cx,
                    ))
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
}

impl Job {
    fn name(&self) -> &str {
        match self {
            Job::Add(new) => &new.name,
            Job::Replace(provider) => &provider.name,
        }
    }
}

impl Render for Providers {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .p_4()
            .gap_4()
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
            .map(|this| match &self.form {
                Some(form) => this.child(self.form(form, cx)),
                None => this.child(
                    h_flex().child(
                        Button::new("add-provider")
                            .outline()
                            .label(t!("Add Provider…").into_owned())
                            .disabled(matches!(self.listing, Listing::Failed(_)))
                            .on_click(cx.listener(|this, _, window, cx| this.start_adding(window, cx))),
                    ),
                ),
            })
    }
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
