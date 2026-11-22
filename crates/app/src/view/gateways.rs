//! Adding a gateway, signing in to one again and removing one: what `openshell gateway add`, `login` and `remove` do,
//! from the sidebar. OpenShell keeps the list, and every call runs off the window's thread, since a sign-in waits for a
//! browser.

use std::collections::BTreeSet;

use playpen_core::{Forgotten, NewGateway, Oidc, Reach};

use gpui_kit::component::switch::Switch;

use super::*;
use crate::providers::ShownGateway;

/// A gateway being added, in place of the chosen sandbox.
pub struct Adding {
    address: Entity<InputState>,
    name: Entity<InputState>,
    /// Whether people sign in to it through an OIDC issuer, which only an `https://` gateway can.
    oidc: bool,
    sign_in: SignInFields,
    state: AddState,
    _changed: Vec<Subscription>,
    _adding: Option<Task<()>>,
}

/// What an OIDC gateway's sign-in is told, typed in.
struct SignInFields {
    issuer: Entity<InputState>,
    client_id: Entity<InputState>,
    audience: Entity<InputState>,
    scopes: Entity<InputState>,
}

impl SignInFields {
    fn all(&self) -> [&Entity<InputState>; 4] {
        [&self.issuer, &self.client_id, &self.audience, &self.scopes]
    }

    fn typed(&self, cx: &App) -> Oidc {
        let value = |field: &Entity<InputState>| field.read(cx).value().to_string();
        Oidc {
            issuer: value(&self.issuer),
            client_id: value(&self.client_id),
            audience: value(&self.audience),
            scopes: value(&self.scopes),
        }
    }
}

enum AddState {
    Choosing,
    Working,
    Failed(String),
}

/// What's being done to a gateway that takes a while, for the sidebar to say.
pub struct GatewayWork {
    said: String,
    _task: Task<()>,
}

impl MainView {
    /// Shows the form for adding a gateway.
    pub(super) fn start_adding_gateway(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.adding.is_some() {
            return;
        }
        let address =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://openshell.example.com"));
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("Made from the address").into_owned())
        });
        let field = |placeholder: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let sign_in = SignInFields {
            issuer: field(
                "https://login.example.com/realms/openshell".to_string(),
                window,
                cx,
            ),
            client_id: field("openshell-cli".to_string(), window, cx),
            audience: field(t!("The same as the client").into_owned(), window, cx),
            scopes: field("openid profile".to_string(), window, cx),
        };
        let mut changed = Vec::new();
        for field in [&address, &name]
            .into_iter()
            .chain(sign_in.all())
            .collect::<Vec<_>>()
        {
            changed.push(cx.subscribe_in(
                field,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.add_gateway(window, cx),
                    // The note under the address says how it's reached, so it follows what's typed.
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ));
        }
        address.read(cx).focus_handle(cx).focus(window, cx);
        self.making = None;
        self.imaging = None;
        self.failed = None;
        self.adding = Some(Adding {
            address,
            name,
            oidc: false,
            sign_in,
            state: AddState::Choosing,
            _changed: changed,
            _adding: None,
        });
        cx.notify();
    }

    fn set_oidc(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(adding) = &mut self.adding {
            adding.oidc = on;
            cx.notify();
        }
    }

    pub(super) fn cancel_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.adding.as_ref().map(|adding| &adding.state),
            Some(AddState::Working)
        ) {
            return;
        }
        self.adding = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Registers the gateway in the form, and shows it once it's there.
    pub(super) fn add_gateway(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let openshell = self.openshell();
        let known: BTreeSet<String> = self
            .gateways
            .iter()
            .map(|gateway| gateway.name.clone())
            .collect();
        let Some(adding) = &mut self.adding else {
            return;
        };
        if matches!(adding.state, AddState::Working) {
            return;
        }
        let new = NewGateway::new(
            &adding.address.read(cx).value(),
            &adding.name.read(cx).value(),
        )
        .and_then(|new| {
            // Only an https:// gateway has the choice; the switch isn't shown for another, and what was typed under it
            // is left out if the address changed after.
            if adding.oidc && new.reach() == Reach::Browser {
                new.with_oidc(adding.sign_in.typed(cx))
            } else {
                Ok(new)
            }
        });
        let new = match new {
            Ok(new) => new,
            Err(error) => {
                adding.state = AddState::Failed(error.to_string());
                cx.notify();
                return;
            }
        };
        tracing::info!(endpoint = new.endpoint(), "adding a gateway");
        adding.state = AddState::Working;
        let done = cx.background_spawn(async move {
            let openshell = openshell?;
            openshell.add_gateway(&new)?;
            openshell.gateways()
        });
        adding._adding = Some(cx.spawn_in(window, async move |this, cx| {
            let done = done.await;
            let _ = this.update_in(cx, |this, window, cx| match done {
                Ok(gateways) => {
                    let added = gateways
                        .iter()
                        .find(|gateway| !known.contains(&gateway.name))
                        .map(|gateway| gateway.name.clone());
                    tracing::info!(?added, "added a gateway");
                    this.adding = None;
                    this.gateways_listed(Ok(gateways));
                    if let Some(name) = added {
                        this.show_gateway(&name, window, cx);
                    }
                    this.focus.focus(window, cx);
                    cx.notify();
                }
                Err(error) => {
                    tracing::warn!("couldn't add a gateway: {error}");
                    if let Some(adding) = &mut this.adding {
                        adding.state = AddState::Failed(error.to_string());
                        cx.notify();
                    }
                }
            });
        }));
        cx.notify();
    }

    /// Asks before forgetting the gateway `name`, which OpenShell does without deleting the gateway or its sandboxes.
    pub(super) fn confirm_remove_gateway(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(gateway) = self
            .gateways
            .iter()
            .find(|gateway| gateway.name == name)
            .cloned()
        else {
            return;
        };
        if gateway.has_certificates_of_its_own() {
            self.failed = Some(
                t!(
                    "“%{name}” is the gateway OpenShell runs on this Mac, so Playpen doesn't remove it. To remove it anyway, run: openshell gateway remove %{name}",
                    name = name
                )
                .into_owned(),
            );
            cx.notify();
            return;
        }
        let remove = t!("Remove");
        let answer = window.prompt(
            PromptLevel::Warning,
            &t!("Remove “%{name}”?", name = name),
            Some(&t!(
                "OpenShell forgets how to reach it, and its certificates are deleted. The gateway keeps running, and so do its sandboxes."
            )),
            &[&*remove, &*t!("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.remove_gateway(gateway, window, cx);
                });
            }
        })
        .detach();
    }

    fn remove_gateway(
        &mut self,
        gateway: GatewayInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let openshell = self.openshell();
        self.failed = None;
        tracing::info!(gateway = %gateway.name, "removing a gateway");
        let name = gateway.name.clone();
        let done = cx.background_spawn(async move {
            let openshell = openshell?;
            let forgotten = openshell.remove_gateway(&gateway)?;
            Ok::<_, OpenShellError>((forgotten, openshell.gateways()?))
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let done = done.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.gateway_working = None;
                match done {
                    Ok((forgotten, gateways)) => {
                        tracing::info!(gateway = %name, "removed a gateway");
                        // What was chosen for it goes too, so a gateway given the name later starts with nothing.
                        crate::settings::set_gateway(&name, None, cx);
                        let was_shown = this.gateway.name() == Some(name.as_str());
                        this.gateways_listed(Ok(gateways));
                        if let Forgotten::Left(folder) = forgotten {
                            this.failed = Some(
                                t!(
                                    "“%{name}” is removed. Its certificates are still in %{folder}.",
                                    name = name,
                                    folder = folder.display().to_string()
                                )
                                .into_owned(),
                            );
                        }
                        if was_shown {
                            // Another gateway is in focus in its place, which `gateways_listed` chose; what was kept
                            // for the one that's gone, and a sandbox chosen on it, went with it.
                            cx.set_global(ShownGateway(this.gateway.clone()));
                        }
                        this.refresh(window, cx);
                    }
                    Err(error) => {
                        tracing::warn!(gateway = %name, "couldn't remove a gateway: {error}");
                        this.failed = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        });
        self.gateway_working = Some(GatewayWork {
            said: t!("Removing the gateway…").into_owned(),
            _task: task,
        });
        cx.notify();
    }

    /// Signs in to the gateway `name` again, which opens a browser and waits for it.
    pub(super) fn sign_in_to_gateway(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.gateway_working.is_some() {
            return;
        }
        let Some(gateway) = self
            .gateways
            .iter()
            .find(|gateway| gateway.name == name)
            .cloned()
        else {
            return;
        };
        let openshell = self.openshell();
        self.failed = None;
        tracing::info!(gateway = %name, "signing in to a gateway");
        let done = cx.background_spawn(async move { openshell?.sign_in_to_gateway(&gateway) });
        let task = cx.spawn_in(window, async move |this, cx| {
            let done = done.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.gateway_working = None;
                if let Err(error) = done {
                    tracing::warn!(gateway = %name, "couldn't sign in to a gateway: {error}");
                    this.failed = Some(error.to_string());
                }
                this.refresh(window, cx);
                cx.notify();
            });
        });
        self.gateway_working = Some(GatewayWork {
            said: t!("Waiting for your browser to sign in…").into_owned(),
            _task: task,
        });
        cx.notify();
    }

    /// What's being done to a gateway, for the sidebar's footer.
    pub(super) fn gateway_work_said(&self) -> Option<&str> {
        self.gateway_working.as_ref().map(|work| work.said.as_str())
    }

    pub(super) fn adding_panel(&self, adding: &Adding, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let working = matches!(adding.state, AddState::Working);
        let typed = adding.address.read(cx).value().to_string();
        let header = Self::panel_header(t!("Add Gateway"), cx).child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("cancel-adding")
                        .ghost()
                        .xsmall()
                        .disabled(working)
                        .label(t!("Cancel").into_owned())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.cancel_adding(window, cx)),
                        ),
                )
                .child(
                    Button::new("add-gateway")
                        .primary()
                        .xsmall()
                        .disabled(working || typed.trim().is_empty())
                        .label(t!("Add").into_owned())
                        .on_click(cx.listener(|this, _, window, cx| this.add_gateway(window, cx))),
                ),
        );
        // What the address says about how the gateway is reached, as it's typed.
        let how = match NewGateway::new(&typed, "").map(|new| new.reach()) {
            Ok(Reach::Browser) => hint(
                t!("OpenShell opens your browser to sign in, and Playpen waits for you."),
                cx,
            ),
            Ok(Reach::Ssh) => hint(
                t!(
                    "Reached over SSH. Its certificates have to be in OpenShell's folder already, which Playpen can't put there."
                ),
                cx,
            ),
            Ok(Reach::PlainHttp) => div().text_sm().text_color(theme.warning).child(
                t!("No encryption and no sign-in. Only for a gateway you're developing.")
                    .into_owned(),
            ),
            Err(_) => hint(t!("An address starting https://, ssh:// or http://."), cx),
        };
        let state = match &adding.state {
            AddState::Choosing => div(),
            AddState::Working => hint(t!("Adding it…"), cx),
            AddState::Failed(error) => div()
                .text_sm()
                .text_color(theme.danger)
                .child(error.clone()),
        };
        // Only an https:// gateway has a sign-in to choose.
        let browser = NewGateway::new(&typed, "").is_ok_and(|new| new.reach() == Reach::Browser);
        let body = v_flex()
            .gap_5()
            .p_4()
            .child(section(t!("Address"), cx).child(div().w(rems(28.)).child(small_field(&adding.address))).child(how))
            .when(browser, |body| {
                let field = |title: String, state: &Entity<InputState>| {
                    section(title, cx).child(div().w(rems(28.)).child(small_field(state)))
                };
                body.child(
                    section(t!("Sign-in"), cx)
                        .child(
                            Switch::new("add-oidc")
                                .label(t!("Sign in through an OIDC issuer").into_owned())
                                .checked(adding.oidc)
                                .disabled(working)
                                .on_click(cx.listener(|this, on: &bool, _, cx| this.set_oidc(*on, cx))),
                        )
                        .child(hint(
                            if adding.oidc {
                                t!("The identity provider the gateway is set up to trust. OpenShell opens your browser to sign in to it.")
                            } else {
                                t!("Without one, the gateway's own sign-in page opens in your browser.")
                            },
                            cx,
                        )),
                )
                .when(adding.oidc, |body| {
                    body.child(field(t!("Issuer").into_owned(), &adding.sign_in.issuer))
                        .child(field(t!("Client ID").into_owned(), &adding.sign_in.client_id))
                        .child(field(t!("Audience").into_owned(), &adding.sign_in.audience))
                        .child(field(t!("Scopes").into_owned(), &adding.sign_in.scopes))
                })
            })
            .child(
                section(t!("Name"), cx)
                    .child(div().w(rems(20.)).child(small_field(&adding.name)))
                    .child(hint(
                        t!("Optional. What OpenShell and Playpen call the gateway, with letters, digits, dots, dashes and underscores."),
                        cx,
                    )),
            )
            .child(state);
        v_flex()
            .size_full()
            .child(header)
            .child(div().id("adding").flex_1().overflow_y_scroll().child(body))
            .into_any_element()
    }
}

#[cfg(test)]
impl Adding {
    /// Why adding failed, if it did.
    pub(super) fn failure(&self) -> Option<&str> {
        match &self.state {
            AddState::Failed(error) => Some(error),
            _ => None,
        }
    }

    /// Types `text` in the address field.
    pub(super) fn type_address(&self, text: &str, window: &mut Window, cx: &mut App) {
        self.address.update(cx, |state, cx| {
            state.set_value(text.to_string(), window, cx)
        });
    }

    /// Types `text` in the OIDC issuer field.
    pub(super) fn type_issuer(&self, text: &str, window: &mut Window, cx: &mut App) {
        self.sign_in.issuer.update(cx, |state, cx| {
            state.set_value(text.to_string(), window, cx)
        });
    }
}

#[cfg(test)]
impl MainView {
    /// Turns the OIDC switch on or off.
    pub(super) fn use_oidc(&mut self, on: bool, cx: &mut Context<Self>) {
        self.set_oidc(on, cx);
    }
}
