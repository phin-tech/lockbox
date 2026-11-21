//! The sandboxes on OpenShell's gateway: listed every few seconds, the chosen one shown beside the list, and
//! opening, starting, stopping and deleting it. Every call to OpenShell runs off the window's thread.

use std::time::Duration;

use playpen_core::{HerdrCore, Installed, Keep, Opened, Phase, session};

use super::*;

/// How often the sandboxes are listed again while the window's open. OpenShell sends nothing when one changes.
const LIST_EVERY: Duration = Duration::from_secs(3);

/// Something done to a sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Act {
    Start,
    /// Starts a sandbox that failed, which takes a stop first.
    Restart,
    Stop,
    Delete,
}

impl MainView {
    /// Lists the sandboxes now, and then every few seconds while the window's open.
    pub(super) fn watch_sandboxes(window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok(openshell) = this.update(cx, |this, _| this.openshell()) else {
                    break;
                };
                let listed = cx
                    .background_spawn(async move {
                        openshell.and_then(|openshell| openshell.sandboxes())
                    })
                    .await;
                // What each agent is doing, when herdr-core keeps their sessions; nothing without it.
                let states = cx
                    .background_spawn(
                        async move { HerdrCore::find().map(|core| core.agent_states()) },
                    )
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.agent_states = match states {
                            Some(Ok(states)) => {
                                this.herdr_failed = None;
                                states
                            }
                            // Logged once, not every few seconds, since it's asked again every few seconds.
                            Some(Err(error)) => {
                                let error = error.to_string();
                                if this.herdr_failed.as_ref() != Some(&error) {
                                    tracing::warn!(
                                        "herdr-core's sessions couldn't be read: {error}"
                                    );
                                    this.herdr_failed = Some(error);
                                }
                                BTreeMap::new()
                            }
                            None => BTreeMap::new(),
                        };
                        this.listed(listed, cx);
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(LIST_EVERY).await;
            }
        })
    }

    /// The `openshell` command, looked for again if it wasn't there before, since it may have been installed since.
    pub(super) fn openshell(&mut self) -> Result<OpenShell, OpenShellError> {
        if self.openshell.is_err() {
            self.openshell = OpenShell::find();
        }
        self.openshell.clone()
    }

    /// Lists the sandboxes again now.
    pub(super) fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.watch == Watch::OpenShell {
            self._watching = Some(Self::watch_sandboxes(window, cx));
        }
    }

    /// Takes in what listing found, keeping the chosen sandbox chosen while it's there.
    pub(super) fn listed(
        &mut self,
        listed: Result<Vec<Sandbox>, OpenShellError>,
        cx: &mut Context<Self>,
    ) {
        self.sandboxes = match listed {
            Ok(sandboxes) => {
                let still_there = self
                    .selected
                    .as_ref()
                    .is_some_and(|name| sandboxes.iter().any(|sandbox| &sandbox.name == name));
                if !still_there {
                    self.selected = sandboxes.first().map(|sandbox| sandbox.name.clone());
                }
                Listing::Listed(sandboxes)
            }
            Err(error) => {
                let error = error.to_string();
                // Logged once, not every few seconds, since it's asked again every few seconds.
                if !matches!(&self.sandboxes, Listing::Failed(before) if *before == error) {
                    tracing::warn!("the sandboxes couldn't be listed: {error}");
                }
                Listing::Failed(error)
            }
        };
        cx.notify();
    }

    fn listed_sandboxes(&self) -> &[Sandbox] {
        match &self.sandboxes {
            Listing::Listed(sandboxes) => sandboxes,
            Listing::Loading | Listing::Failed(_) => &[],
        }
    }

    pub(super) fn selected_sandbox(&self) -> Option<&Sandbox> {
        let name = self.selected.as_ref()?;
        self.listed_sandboxes()
            .iter()
            .find(|sandbox| &sandbox.name == name)
    }

    pub(super) fn select(&mut self, name: String, cx: &mut Context<Self>) {
        self.selected = Some(name);
        self.making = None;
        cx.notify();
    }

    /// Chooses the sandbox `step` places down the list from the chosen one, or up for a negative step.
    pub(super) fn select_next(&mut self, step: isize, cx: &mut Context<Self>) {
        let sandboxes = self.listed_sandboxes();
        if sandboxes.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|name| sandboxes.iter().position(|sandbox| &sandbox.name == name));
        let next = match at {
            Some(at) => at.saturating_add_signed(step).min(sandboxes.len() - 1),
            None => 0,
        };
        let name = sandboxes[next].name.clone();
        self.select(name, cx);
    }

    pub(super) fn act_on_selected(
        &mut self,
        act: Act,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sandbox) = self.selected_sandbox() else {
            return;
        };
        let can = match act {
            Act::Start => sandbox.phase.can_start(),
            Act::Restart => sandbox.phase.needs_restart(),
            Act::Stop => sandbox.phase.can_stop(),
            Act::Delete => true,
        };
        if can {
            let name = sandbox.name.clone();
            self.act(name, act, window, cx);
        }
    }

    /// Asks before deleting the chosen sandbox, whose files go with it.
    pub(super) fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.selected_sandbox().map(|sandbox| sandbox.name.clone()) else {
            return;
        };
        let delete = t!("Delete");
        let answer = window.prompt(
            PromptLevel::Warning,
            &t!("Delete “%{name}”?", name = name),
            Some(&t!(
                "Its files go with it, and anything its agent made that you haven't copied out."
            )),
            &[&*delete, &*t!("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.act(name, Act::Delete, window, cx);
                });
            }
        })
        .detach();
    }

    /// Starts, stops or deletes the sandbox `name`, off the window's thread, and lists the sandboxes again after.
    fn act(&mut self, name: String, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        if self.working.contains_key(&name) {
            return;
        }
        let openshell = match self.openshell() {
            Ok(openshell) => openshell,
            Err(error) => {
                self.failed = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        self.failed = None;
        tracing::info!(sandbox = %name, ?act, "acting on a sandbox");
        let doing = name.clone();
        let done = cx.background_spawn(async move {
            match act {
                Act::Start => openshell.start(&doing),
                Act::Restart => openshell.restart(&doing),
                Act::Stop => openshell.stop(&doing),
                Act::Delete => openshell.delete(&doing),
            }
        });
        let task = cx.spawn_in(window, {
            let name = name.clone();
            async move |this, cx| {
                let done = done.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.working.remove(&name);
                    match done {
                        Ok(()) => tracing::info!(sandbox = %name, ?act, "done"),
                        Err(error) => {
                            tracing::warn!(sandbox = %name, ?act, "failed: {error}");
                            this.failed = Some(error.to_string());
                        }
                    }
                    this.refresh(window, cx);
                    cx.notify();
                });
            }
        });
        self.working.insert(name, task);
        cx.notify();
    }

    /// Opens a session running `what` in the chosen sandbox, shown in `place`, or where the settings say. herdr-core
    /// keeps it when it's installed and the settings ask; one it keeps that's open already is shown again.
    pub(super) fn open_session(
        &mut self,
        place: Option<Place>,
        what: What,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sandbox) = self.selected_sandbox().cloned() else {
            return;
        };
        if !sandbox.phase.is_ready() || self.working.contains_key(&sandbox.name) {
            return;
        }
        let openshell = match self.openshell() {
            Ok(openshell) => openshell,
            Err(error) => {
                self.failed = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        self.failed = None;
        let settings = crate::settings::current(cx);
        let keep = if settings.keep_sessions {
            Keep::InHerdrCore
        } else {
            Keep::Nowhere
        };
        let name = sandbox.name.clone();
        tracing::info!(sandbox = %name, ?what, ?place, ?keep, "opening a session");
        let opened = cx.background_spawn(async move {
            let place = place.unwrap_or_else(|| settings.show_in.place(&Installed::now()));
            session::open(openshell.binary(), &sandbox, what, place, keep)
        });
        let task = cx.spawn_in(window, {
            let name = name.clone();
            async move |this, cx| {
                let opened = opened.await;
                let _ = this.update(cx, |this, cx| {
                    this.working.remove(&name);
                    match &opened {
                        Ok(Opened::AsAsked) => tracing::info!(sandbox = %name, "opened a session"),
                        Ok(Opened::InTerminalInstead(place)) => {
                            tracing::info!(sandbox = %name, ?place, "not installed, so opened in Terminal");
                        }
                        Err(error) => tracing::warn!(sandbox = %name, "couldn't open a session: {error}"),
                    }
                    this.failed = match opened {
                        Ok(Opened::AsAsked) => None,
                        Ok(Opened::InTerminalInstead(place)) => Some(
                            t!(
                                "%{place} isn't installed, so it opened in Terminal.",
                                place = place_name(place)
                            )
                            .into_owned(),
                        ),
                        Err(error) => Some(
                            t!(
                                "Couldn't open “%{name}”: %{error}",
                                name = name,
                                error = error.to_string()
                            )
                            .into_owned(),
                        ),
                    };
                    cx.notify();
                });
            }
        });
        self.working.insert(name, task);
        cx.notify();
    }

    pub(super) fn sandbox_list(&self, cx: &Context<Self>) -> Div {
        let header = Self::panel_header(t!("Sandboxes"), cx).child(
            Button::new("new-agent")
                .ghost()
                .xsmall()
                .label(t!("New Agent…").into_owned())
                .on_click(cx.listener(|this, _, window, cx| this.run("sandbox.new", window, cx))),
        );
        let rows = self.listed_sandboxes().iter().map(|sandbox| {
            let chosen = self.selected.as_ref() == Some(&sandbox.name) && self.making.is_none();
            let name = sandbox.name.clone();
            list_row(chosen, cx)
                .id(ElementId::from((ElementId::from("sandbox"), name.clone())))
                .justify_between()
                .child(div().min_w_0().truncate().child(sandbox.name.clone()))
                .child(state_badge(
                    &sandbox.phase,
                    self.agent_states.get(&sandbox.name).copied(),
                ))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select(name.clone(), cx);
                    this.focus.focus(window, cx);
                }))
        });
        let mut footer = Footer::new()
            .busy(!self.working.is_empty() || matches!(self.sandboxes, Listing::Loading));
        match &self.sandboxes {
            Listing::Loading => footer = footer.line(t!("Asking OpenShell…").into_owned()),
            Listing::Failed(error) => footer = footer.error(error.clone()),
            Listing::Listed(sandboxes) => {
                footer = footer.line(match sandboxes.len() {
                    0 => t!("No sandboxes").into_owned(),
                    1 => t!("One sandbox").into_owned(),
                    count => t!("%{count} sandboxes", count = count).into_owned(),
                });
            }
        }
        if let Some(failed) = &self.failed {
            footer = footer.error(failed.clone());
        }
        v_flex()
            .size_full()
            .child(header)
            .child(
                div()
                    .id("sandboxes")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .child(footer)
    }

    pub(super) fn sandbox_detail(&self, cx: &Context<Self>) -> AnyElement {
        let Some(sandbox) = self.selected_sandbox() else {
            return self.nothing_chosen(cx).into_any_element();
        };
        let busy = self.working.contains_key(&sandbox.name);
        let agent = sandbox.agent.as_deref().and_then(Agent::from_id);
        let header =
            Self::panel_header(sandbox.name.clone(), cx).child(phase_badge(&sandbox.phase));
        // They wrap rather than run out of a narrow window.
        let actions = h_flex()
            .flex_wrap()
            .gap_2()
            .child(open_button(
                agent,
                busy || !sandbox.phase.is_ready(),
                Installed::now(),
                cx,
            ))
            .child(if sandbox.phase.needs_restart() {
                Button::new("restart")
                    .small()
                    .disabled(busy)
                    .label(t!("Restart").into_owned())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.restart", window, cx)),
                    )
            } else if sandbox.phase.can_start() {
                Button::new("start")
                    .small()
                    .disabled(busy)
                    .label(t!("Start").into_owned())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.start", window, cx)),
                    )
            } else {
                Button::new("stop")
                    .small()
                    .disabled(busy || !sandbox.phase.can_stop())
                    .label(t!("Stop").into_owned())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.stop", window, cx)),
                    )
            })
            .child(
                Button::new("delete")
                    .small()
                    .ghost()
                    .disabled(busy)
                    .label(t!("Delete…").into_owned())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.delete", window, cx)),
                    ),
            );
        let body = v_flex()
            .gap_5()
            .p_4()
            .child(actions)
            .child(section(t!("Agent"), cx).child(match agent {
                Some(agent) => div().child(agent.name()),
                None => hint(t!("Not one Playpen made"), cx),
            }))
            .when_some(sandbox.folder.clone(), |body, folder| {
                body.child(section(t!("Copied from"), cx).child(h_flex().child(shell::code_chip(folder, cx))))
            })
            .when_some(sandbox.workdir.clone(), |body, workdir| {
                body.child(section(t!("Works in"), cx).child(h_flex().child(shell::code_chip(workdir, cx))))
            })
            .child(section(t!("Made"), cx).child(div().child(t!("%{when} UTC", when = sandbox.created.clone()).into_owned())))
            .child(hint(
                t!(
                    "Its agent reaches its model and nothing else. Stopping keeps its files; deleting doesn't."
                ),
                cx,
            ));
        v_flex()
            .size_full()
            .child(header)
            .child(div().id("sandbox").flex_1().overflow_y_scroll().child(body))
            .into_any_element()
    }

    /// The detail panel with no sandbox to show: why, and how to make one.
    fn nothing_chosen(&self, cx: &Context<Self>) -> Div {
        let (title, said) = match &self.sandboxes {
            Listing::Loading => (t!("Sandboxes"), t!("Asking OpenShell for its sandboxes…")),
            Listing::Failed(_) => (
                t!("OpenShell isn't answering"),
                t!(
                    "Is its gateway running? `brew services start openshell` starts it. Playpen asks again every few seconds."
                ),
            ),
            Listing::Listed(_) => (
                t!("No sandboxes yet"),
                t!(
                    "Make one for an agent with New Agent…: it runs in its own sandbox, on a copy of a folder, and reaches its model and nothing else."
                ),
            ),
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .p_6()
            .child(div().text_lg().font_semibold().child(title.into_owned()))
            .child(
                div()
                    .max_w(rems(28.))
                    .text_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(said.into_owned()),
            )
            .when(matches!(self.sandboxes, Listing::Listed(_)), |this| {
                this.child(
                    Button::new("new-agent-empty")
                        .primary()
                        .small()
                        .label(t!("New Agent…").into_owned())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.run("sandbox.new", window, cx)),
                        ),
                )
            })
    }
}

/// A row of the list: the chosen one in the list's selection colour.
fn list_row(chosen: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let hover = theme.list_hover;
    h_flex()
        .gap_2()
        .px_3()
        .py_2()
        .cursor_pointer()
        .when(chosen, |row| row.bg(theme.list_active))
        .when(!chosen, |row| row.hover(move |row| row.bg(hover)))
}

fn phase_badge(phase: &Phase) -> Tag {
    let (tag, words) = match phase {
        Phase::Ready => (Tag::success(), t!("Ready").into_owned()),
        Phase::Provisioning => (Tag::info(), t!("Starting").into_owned()),
        Phase::Stopped => (Tag::secondary(), t!("Stopped").into_owned()),
        Phase::Suspended => (Tag::secondary(), t!("Suspended").into_owned()),
        Phase::Completed => (Tag::secondary(), t!("Finished").into_owned()),
        Phase::Deleting => (Tag::warning(), t!("Deleting").into_owned()),
        Phase::Error => (Tag::danger(), t!("Failed").into_owned()),
        Phase::Other(name) => (Tag::secondary(), name.clone()),
    };
    shell::badge(tag).xsmall().child(words)
}

/// A place's name, as the menus give it.
fn place_name(place: Place) -> String {
    match place {
        Place::HerdrApp => t!("The Herdr app").into_owned(),
        Place::Ghostty => t!("Ghostty").into_owned(),
        Place::Terminal => t!("Terminal").into_owned(),
    }
}

/// What the agent in a sandbox's sessions is doing, as a badge, or its phase when no session says.
fn state_badge(phase: &Phase, agent: Option<AgentState>) -> Tag {
    match (phase, agent) {
        (Phase::Ready, Some(state)) => {
            let (tag, words) = match state {
                AgentState::Working => (Tag::info(), t!("Working")),
                AgentState::Blocked => (Tag::warning(), t!("Waiting for you")),
                AgentState::Done => (Tag::success(), t!("Done")),
                AgentState::Idle => (Tag::secondary(), t!("Idle")),
            };
            shell::badge(tag).xsmall().child(words.into_owned())
        }
        _ => phase_badge(phase),
    }
}

/// A menu item's action: the Command `name`.
fn run(name: &'static str) -> Box<dyn Action> {
    Box::new(RunAction { name })
}

/// Open, which opens a session where the settings say, with the rest of the choices for one session under its arrow.
/// A place that isn't installed is there, but can't be chosen.
fn open_button(
    agent: Option<Agent>,
    disabled: bool,
    installed: Installed,
    cx: &Context<MainView>,
) -> DropdownButton {
    let label = match agent {
        Some(agent) => t!("Open %{agent}", agent = agent.name()).into_owned(),
        None => t!("Open Shell").into_owned(),
    };
    DropdownButton::new("open")
        .primary()
        .small()
        .disabled(disabled)
        .button(
            Button::new("open-session")
                .label(label)
                .on_click(cx.listener(|this, _, window, cx| this.run("sandbox.open", window, cx))),
        )
        .dropdown_menu(move |menu, _, _| {
            let menu = if agent.is_some() {
                menu.menu(
                    t!("Continue Last Conversation").into_owned(),
                    run("sandbox.continue"),
                )
                .menu(t!("Open Shell").into_owned(), run("sandbox.shell"))
                .separator()
            } else {
                menu
            };
            menu.menu_with_disabled(
                t!("Open in Herdr App").into_owned(),
                run("sandbox.open-herdr"),
                !installed.herdr_app,
            )
            .menu_with_disabled(
                t!("Open in Ghostty").into_owned(),
                run("sandbox.open-ghostty"),
                !installed.ghostty,
            )
            .menu(
                t!("Open in Terminal").into_owned(),
                run("sandbox.open-terminal"),
            )
        })
}
