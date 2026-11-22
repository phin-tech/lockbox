//! The chosen sandbox's forwarded ports: a port inside it opened on this Mac's loopback at the same number, so what it
//! serves opens in the Mac's browser. Playpen keeps which forward is whose in `forwards.json`, since OpenShell lists
//! them with no gateway, and checks them against OpenShell's list when a sandbox is chosen.

use playpen_core::{Forward, Forwards};

use super::*;

/// The forwards Playpen started, and the port being typed.
pub(super) struct Ports {
    forwards: Forwards,
    /// Where they're kept, or nowhere in a test or a capture.
    file: Option<std::path::PathBuf>,
    pub(super) field: Entity<InputState>,
    /// The port being forwarded or stopped, while it is.
    working: Option<u16>,
    _working: Option<Task<()>>,
    _checking: Option<Task<()>>,
    _typed: Subscription,
}

impl Ports {
    pub(super) fn new(watch: Watch, window: &mut Window, cx: &mut Context<MainView>) -> Ports {
        let field = cx.new(|cx| InputState::new(window, cx).placeholder("3000"));
        let typed = cx.subscribe_in(&field, window, |this, _, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.forward_typed_port(cx);
            }
        });
        let file = (watch == Watch::OpenShell)
            .then(|| crate::settings::file("forwards.json"))
            .flatten();
        Ports {
            forwards: file.as_deref().map(Forwards::load).unwrap_or_default(),
            file,
            field,
            working: None,
            _working: None,
            _checking: None,
            _typed: typed,
        }
    }

    fn save(&self) {
        if let Some(file) = &self.file
            && let Err(error) = self.forwards.save(file)
        {
            tracing::warn!("couldn't keep the forwarded ports: {error}");
        }
    }

    /// The forwards of the sandbox `key`.
    pub(super) fn of(&self, key: &SandboxKey) -> Vec<Forward> {
        self.forwards.of(&key.gateway, &key.name).cloned().collect()
    }
}

impl MainView {
    /// Forgets forwards OpenShell no longer runs, such as those of a sandbox deleted since. A test or a capture
    /// doesn't ask.
    pub(super) fn check_forwards(&mut self, cx: &mut Context<Self>) {
        if self.watch == Watch::Nothing || self.ports.forwards.forwards.is_empty() {
            return;
        }
        let Ok(openshell) = self.base_openshell() else {
            return;
        };
        let listed = cx.background_spawn(async move { openshell.forwards() });
        self.ports._checking = Some(cx.spawn(async move |this, cx| {
            let Ok(running) = listed.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let before = this.ports.forwards.forwards.len();
                this.ports.forwards.keep_running(&running);
                if this.ports.forwards.forwards.len() != before {
                    this.ports.save();
                    cx.notify();
                }
            });
        }));
    }

    /// Forwards the port typed to the chosen sandbox.
    fn forward_typed_port(&mut self, cx: &mut Context<Self>) {
        let typed = self.ports.field.read(cx).value().trim().to_string();
        match typed.parse::<u16>() {
            Ok(port) if port > 0 => self.forward_port(port, cx),
            _ => {
                self.failed = Some(
                    t!(
                        "“%{typed}” isn't a port: a number from 1 to 65535.",
                        typed = typed
                    )
                    .into_owned(),
                );
                cx.notify();
            }
        }
    }

    /// Forwards `port` to the chosen sandbox, off the window's thread.
    pub(super) fn forward_port(&mut self, port: u16, cx: &mut Context<Self>) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        if self.ports.working.is_some() {
            return;
        }
        if self
            .ports
            .of(&key)
            .iter()
            .any(|forward| forward.port == port)
        {
            return;
        }
        let Ok(openshell) = self.openshell_for(&key.gateway) else {
            return;
        };
        tracing::info!(sandbox = %key.name, port, "forwarding a port");
        self.failed = None;
        self.ports.working = Some(port);
        let name = key.name.clone();
        let done = cx.background_spawn(async move { openshell.forward_port(&name, port) });
        self.ports._working = Some(cx.spawn(async move |this, cx| {
            let done = done.await;
            let _ = this.update(cx, |this, cx| {
                this.ports.working = None;
                match done {
                    Ok(()) => {
                        this.ports.forwards.forwards.push(Forward {
                            gateway: key.gateway.clone(),
                            sandbox: key.name.clone(),
                            port,
                        });
                        this.ports.forwards.forwards.sort();
                        this.ports.save();
                    }
                    Err(error) => {
                        tracing::warn!("couldn't forward a port: {error}");
                        this.failed = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Stops forwarding `forward`'s port.
    fn stop_forward(&mut self, forward: Forward, cx: &mut Context<Self>) {
        if self.ports.working.is_some() {
            return;
        }
        let Ok(openshell) = self.openshell_for(&forward.gateway) else {
            return;
        };
        self.failed = None;
        self.ports.working = Some(forward.port);
        let (name, port) = (forward.sandbox.clone(), forward.port);
        let done = cx.background_spawn(async move { openshell.stop_forward(&name, port) });
        self.ports._working = Some(cx.spawn(async move |this, cx| {
            let done = done.await;
            let _ = this.update(cx, |this, cx| {
                this.ports.working = None;
                if let Err(error) = done {
                    tracing::warn!("couldn't stop forwarding a port: {error}");
                    this.failed = Some(error.to_string());
                }
                // Gone either way: one OpenShell had stopped already can't be stopped again.
                this.ports.forwards.forwards.retain(|kept| kept != &forward);
                this.ports.save();
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Puts the cursor in the port field, for Forward a Port….
    pub(super) fn focus_port_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ports.field.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// The chosen sandbox's Ports row: each forward with Open and Stop, and a port to forward.
    pub(super) fn ports_row(&self, sandbox: &Sandbox, key: &SandboxKey, cx: &Context<Self>) -> Div {
        let ready = sandbox.phase.is_ready();
        let working = self.ports.working;
        let forwards = self.ports.of(key);
        v_flex()
            .min_w_0()
            .gap_1()
            .items_end()
            .children(forwards.into_iter().map(|forward| {
                let id = |what: &'static str| {
                    ElementId::from((ElementId::from(what), forward.port.to_string()))
                };
                let address = forward.address();
                let stopping = forward.clone();
                h_flex()
                    .gap_1()
                    .child(shell::code_chip(format!("localhost:{}", forward.port), cx))
                    .child(
                        Button::new(id("open-port"))
                            .ghost()
                            .xsmall()
                            .label(t!("Open").into_owned())
                            .on_click(move |_, _, cx| cx.open_url(&address)),
                    )
                    .child(
                        Button::new(id("stop-port"))
                            .ghost()
                            .xsmall()
                            .icon(assets::IconName::X)
                            .tooltip(t!("Stop Forwarding").into_owned())
                            .loading(working == Some(forward.port))
                            .disabled(working.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.stop_forward(stopping.clone(), cx);
                            })),
                    )
            }))
            .child(
                h_flex()
                    .gap_1()
                    .child(div().w(rems(5.)).child(small_field(&self.ports.field)))
                    .child(
                        Button::new("forward-port")
                            .xsmall()
                            .icon(assets::IconName::Plus)
                            .label(t!("Forward").into_owned())
                            .disabled(!ready || working.is_some())
                            .loading(working.is_some_and(|port| {
                                !self
                                    .ports
                                    .of(key)
                                    .iter()
                                    .any(|forward| forward.port == port)
                            }))
                            .on_click(cx.listener(|this, _, _, cx| this.forward_typed_port(cx))),
                    ),
            )
    }
}

#[cfg(test)]
impl MainView {
    /// Records a forward as if it had been started.
    pub(super) fn give_forward(&mut self, forward: Forward) {
        self.ports.forwards.forwards.push(forward);
    }

    pub(super) fn forwards_of(&self, key: &SandboxKey) -> Vec<u16> {
        self.ports
            .of(key)
            .iter()
            .map(|forward| forward.port)
            .collect()
    }
}

impl MainView {
    /// Gives a capture's sandbox `key` some forwarded ports to show.
    pub(super) fn ports_give_stand_ins(&mut self, key: &SandboxKey) {
        self.ports.forwards.forwards.extend(stand_in_forwards(key));
    }
}

/// Forwards for a capture.
fn stand_in_forwards(key: &SandboxKey) -> Vec<Forward> {
    [3000, 5173]
        .into_iter()
        .map(|port| Forward {
            gateway: key.gateway.clone(),
            sandbox: key.name.clone(),
            port,
        })
        .collect()
}
