//! The sandboxes on OpenShell's gateway: listed every few seconds, the chosen one shown beside the list, and
//! opening, starting, stopping and deleting it. Every call to OpenShell runs off the window's thread.

use std::time::Duration;

use gpui_kit::component::Icon;
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
            Ok(mut sandboxes) => {
                // The running ones first, as the list shows them, so the arrows go down it in the order it's seen.
                sandboxes.sort_by_key(|sandbox| !is_running(&sandbox.phase));
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

    /// The list's heading, in the title bar after the traffic lights: what's listed, how many are running, and New
    /// Agent….
    pub(super) fn list_heading(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let said = match &self.sandboxes {
            Listing::Loading => t!("Asking OpenShell…").into_owned(),
            Listing::Failed(_) => t!("OpenShell isn't answering").into_owned(),
            Listing::Listed(sandboxes) => {
                let running = sandboxes
                    .iter()
                    .filter(|sandbox| sandbox.phase.can_stop())
                    .count();
                match (sandboxes.len(), running) {
                    (0, _) => t!("No sandboxes").into_owned(),
                    (_, 0) => t!("None running").into_owned(),
                    (_, running) => t!("%{count} running", count = running).into_owned(),
                }
            }
        };
        h_flex()
            .flex_none()
            .h_full()
            .w(rems(LIST_WIDTH))
            .pl(TRAFFIC_LIGHTS)
            .pr_2()
            .gap_2()
            .justify_between()
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .font_semibold()
                            .child(t!("Sandboxes").into_owned()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(said),
                    ),
            )
            .child(
                Button::new("new-agent")
                    .ghost()
                    .small()
                    .rounded(px(999.))
                    .icon(assets::IconName::Plus)
                    .tooltip(t!("New Agent…").into_owned())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.new", window, cx)),
                    ),
            )
    }

    /// The sandboxes as a sidebar: the running ones, then the rest under a heading of their own, as each is listed.
    pub(super) fn sandbox_list(&self, cx: &Context<Self>) -> Div {
        let sandboxes = self.listed_sandboxes();
        let (running, resting): (Vec<_>, Vec<_>) = sandboxes
            .iter()
            .partition(|sandbox| is_running(&sandbox.phase));
        let rows = |list: Vec<&Sandbox>| {
            list.into_iter()
                .map(|sandbox| self.sandbox_row(sandbox, cx))
                .collect::<Vec<_>>()
        };
        let mut footer = Footer::new()
            .busy(!self.working.is_empty() || matches!(self.sandboxes, Listing::Loading));
        match &self.sandboxes {
            Listing::Loading => footer = footer.line(t!("Asking OpenShell…").into_owned()),
            Listing::Failed(error) => footer = footer.error(error.clone()),
            Listing::Listed(_) => {}
        }
        if let Some(failed) = &self.failed {
            footer = footer.error(failed.clone());
        }
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .id("sandboxes")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .pt_1()
                    .gap(px(2.))
                    .children(rows(running))
                    .when(!resting.is_empty(), |list| {
                        list.child(
                            div()
                                .px_2()
                                .pt_4()
                                .pb_1()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(t!("Stopped").into_owned()),
                        )
                        .children(rows(resting))
                    }),
            )
            .when(!footer.is_empty(), |list| list.child(footer))
    }

    /// A sandbox in the list: its agent's mark in the colour of its state, its name, what it's doing, and a button
    /// each to start or stop it and to delete it.
    fn sandbox_row(&self, sandbox: &Sandbox, cx: &Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let chosen = self.selected.as_ref() == Some(&sandbox.name) && self.making.is_none();
        let busy = self.working.contains_key(&sandbox.name);
        let agent = sandbox.agent.as_deref().and_then(Agent::from_id);
        let state = self.agent_states.get(&sandbox.name).copied();
        let resting = !is_running(&sandbox.phase);
        let name = sandbox.name.clone();
        let id = |what: &str| {
            ElementId::from((
                ElementId::from(SharedString::from(what.to_string())),
                name.clone(),
            ))
        };
        let (act, icon, tip) = if sandbox.phase.needs_restart() {
            ("sandbox.restart", assets::IconName::RotateCw, t!("Restart"))
        } else if sandbox.phase.can_start() {
            ("sandbox.start", assets::IconName::CirclePlay, t!("Start"))
        } else {
            ("sandbox.stop", assets::IconName::CircleStop, t!("Stop"))
        };
        let acts = h_flex()
            .flex_none()
            .gap_0p5()
            .child(
                Button::new(id("row-act"))
                    .ghost()
                    .xsmall()
                    .icon(icon)
                    .tooltip(tip.into_owned())
                    .disabled(busy || (act == "sandbox.stop" && !sandbox.phase.can_stop()))
                    .on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, window, cx| {
                            this.select(name.clone(), cx);
                            this.run(act, window, cx);
                        }
                    })),
            )
            .child(
                Button::new(id("row-delete"))
                    .ghost()
                    .xsmall()
                    .icon(assets::IconName::Trash)
                    .tooltip(t!("Delete…").into_owned())
                    .disabled(busy)
                    .on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, window, cx| {
                            this.select(name.clone(), cx);
                            this.run("sandbox.delete", window, cx);
                        }
                    })),
            );
        let blocked = state == Some(AgentState::Blocked) && sandbox.phase.is_ready();
        let hover = theme.list_hover;
        h_flex()
            .id(id("sandbox"))
            .flex_none()
            .gap_2p5()
            .px_2()
            .py_1p5()
            .rounded_lg()
            .cursor_pointer()
            .when(chosen, |row| row.bg(theme.list_active))
            .when(!chosen, |row| row.hover(move |row| row.bg(hover)))
            .child(agent_tile(
                agent,
                state_colour(&sandbox.phase, state, cx),
                rems(28. / 13.),
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .font_medium()
                            .when(resting, |name| name.text_color(theme.muted_foreground))
                            .child(sandbox.name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(if blocked {
                                theme.warning
                            } else {
                                theme.muted_foreground
                            })
                            .child(
                                t!(
                                    "%{agent} · %{state}",
                                    agent = agent_name(agent),
                                    state = state_words(&sandbox.phase, state)
                                )
                                .into_owned(),
                            ),
                    ),
            )
            .child(acts)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select(name.clone(), cx);
                this.focus.focus(window, cx);
            }))
    }

    pub(super) fn sandbox_detail(&self, cx: &Context<Self>) -> AnyElement {
        let Some(sandbox) = self.selected_sandbox() else {
            return self.nothing_chosen(cx).into_any_element();
        };
        let theme = cx.theme();
        let busy = self.working.contains_key(&sandbox.name);
        let agent = sandbox.agent.as_deref().and_then(Agent::from_id);
        let state = self.agent_states.get(&sandbox.name).copied();
        let hero = h_flex()
            .gap_3()
            .child(agent_tile(
                agent,
                state_colour(&sandbox.phase, state, cx),
                rems(44. / 13.),
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_lg()
                            .font_semibold()
                            .child(sandbox.name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(agent_name(agent)),
                    ),
            );
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
                    .icon(assets::IconName::RotateCw)
                    .label(t!("Restart").into_owned())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.restart", window, cx)),
                    )
            } else if sandbox.phase.can_start() {
                Button::new("start")
                    .small()
                    .disabled(busy)
                    .icon(assets::IconName::CirclePlay)
                    .label(t!("Start").into_owned())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.start", window, cx)),
                    )
            } else {
                Button::new("stop")
                    .small()
                    .disabled(busy || !sandbox.phase.can_stop())
                    .icon(assets::IconName::CircleStop)
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
                    .icon(assets::IconName::Trash)
                    .label(t!("Delete…").into_owned())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.delete", window, cx)),
                    ),
            );
        let mut info = vec![
            (t!("Agent").into_owned(), text_value(agent_name(agent))),
            (
                t!("Status").into_owned(),
                text_value(state_words(&sandbox.phase, state)),
            ),
        ];
        if !sandbox.folders.is_empty() {
            info.push((
                t!("Copied from").into_owned(),
                v_flex().min_w_0().gap_1().items_end().children(
                    sandbox
                        .folders
                        .iter()
                        .map(|folder| shell::code_chip(folder.clone(), cx)),
                ),
            ));
        }
        if let Some(workdir) = sandbox.workdir.clone() {
            info.push((t!("Works in").into_owned(), shell::code_chip(workdir, cx)));
        }
        info.push((
            t!("Made").into_owned(),
            text_value(t!("%{when} UTC", when = sandbox.created.clone()).into_owned()),
        ));
        let body = v_flex()
            .gap_5()
            .p_5()
            .child(hero)
            .child(actions)
            .child(info_card(info, cx))
            .child(hint(
                t!(
                    "Its agent reaches its model and nothing else. Stopping keeps its files; deleting doesn't."
                ),
                cx,
            ));
        div()
            .id("sandbox")
            .size_full()
            .overflow_y_scroll()
            .child(body)
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

/// Sandboxes for a capture to show, of every agent and in each state the list sets apart.
pub(super) fn stand_ins() -> Vec<Sandbox> {
    let sandbox = |name: &str, phase: Phase, agent: Option<&str>| Sandbox {
        name: name.to_string(),
        phase,
        created: "2026-11-21 09:41:00".to_string(),
        agent: agent.map(str::to_string),
        folders: agent
            .map(|_| vec!["/Users/sam/src/Patchbay".to_string()])
            .unwrap_or_default(),
        workdir: agent.map(|_| "/sandbox/Patchbay".to_string()),
    };
    vec![
        sandbox("claude-patchbay", Phase::Ready, Some("claude-code")),
        sandbox("codex-patchbay", Phase::Provisioning, Some("codex")),
        sandbox("scratch", Phase::Error, None),
        sandbox("claude-typecase", Phase::Stopped, Some("claude-code")),
        sandbox("codex-red-pen", Phase::Stopped, Some("codex")),
    ]
}

/// Whether a sandbox is up, or on its way up or down, rather than resting: the list shows these first.
pub(super) fn is_running(phase: &Phase) -> bool {
    !matches!(phase, Phase::Stopped | Phase::Suspended | Phase::Completed)
}

/// An agent's name, or what a sandbox Playpen didn't make offers instead.
fn agent_name(agent: Option<Agent>) -> String {
    match agent {
        Some(agent) => agent.name().to_string(),
        None => t!("Shell").into_owned(),
    }
}

/// The mark for an agent, or a plain box for a sandbox Playpen didn't make.
fn agent_icon(agent: Option<Agent>) -> assets::IconName {
    match agent {
        Some(Agent::ClaudeCode) => assets::IconName::Asterisk,
        Some(Agent::Codex) => assets::IconName::SquareTerminal,
        None => assets::IconName::Box,
    }
}

/// A rounded square holding an agent's mark, tinted with the colour of its sandbox's state, so the list says at a
/// glance which are running, which want the person, and which failed.
fn agent_tile(agent: Option<Agent>, colour: Hsla, size: Rems, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_none()
        .size(size)
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.radius * 1.5)
        .bg(colour.opacity(0.16))
        .border_1()
        .border_color(colour.opacity(0.3))
        .text_color(colour)
        .child(Icon::new(agent_icon(agent)).size(size * 0.55))
}

/// The colour of a sandbox's state: what its agent's doing while it runs, or its phase.
fn state_colour(phase: &Phase, agent: Option<AgentState>, cx: &App) -> Hsla {
    let theme = cx.theme();
    match (phase, agent) {
        (Phase::Ready, Some(AgentState::Working)) => theme.info,
        (Phase::Ready, Some(AgentState::Blocked)) => theme.warning,
        (Phase::Ready, _) => theme.success,
        (Phase::Provisioning, _) => theme.info,
        (Phase::Deleting, _) => theme.warning,
        (Phase::Error, _) => theme.danger,
        (Phase::Stopped | Phase::Suspended | Phase::Completed | Phase::Other(_), _) => {
            theme.muted_foreground
        }
    }
}

/// What a sandbox is doing, in words: its agent's state while it runs, or its phase.
fn state_words(phase: &Phase, agent: Option<AgentState>) -> String {
    match (phase, agent) {
        (Phase::Ready, Some(state)) => match state {
            AgentState::Working => t!("Working"),
            AgentState::Blocked => t!("Waiting for you"),
            AgentState::Done => t!("Done"),
            AgentState::Idle => t!("Idle"),
        }
        .into_owned(),
        _ => phase_words(phase),
    }
}

/// A value in an info card, in plain text.
fn text_value(text: impl Into<SharedString>) -> Div {
    div().min_w_0().truncate().child(text.into())
}

/// Facts about something, in a rounded card set back from the panel: a row each, its name on the left and its value
/// on the right, with a hairline between them.
fn info_card(rows: Vec<(String, Div)>, cx: &App) -> Div {
    let theme = cx.theme();
    let count = rows.len();
    v_flex()
        .rounded(theme.radius * 1.5)
        .border_1()
        .border_color(theme.border)
        .bg(shell::recessed_colour(theme))
        .text_sm()
        .children(rows.into_iter().enumerate().map(|(at, (name, value))| {
            h_flex()
                .gap_4()
                .justify_between()
                .px_3()
                .py_2()
                .when(at + 1 < count, |row| {
                    row.border_b_1().border_color(shell::hairline(theme))
                })
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(name),
                )
                .child(h_flex().min_w_0().justify_end().child(value))
        }))
}

/// A phase in words.
fn phase_words(phase: &Phase) -> String {
    match phase {
        Phase::Ready => t!("Ready").into_owned(),
        Phase::Provisioning => t!("Starting").into_owned(),
        Phase::Stopped => t!("Stopped").into_owned(),
        Phase::Suspended => t!("Suspended").into_owned(),
        Phase::Completed => t!("Finished").into_owned(),
        Phase::Deleting => t!("Deleting").into_owned(),
        Phase::Error => t!("Failed").into_owned(),
        Phase::Other(name) => name.clone(),
    }
}

/// A place's name, as the menus give it.
fn place_name(place: Place) -> String {
    match place {
        Place::HerdrApp => t!("The Herdr app").into_owned(),
        Place::Ghostty => t!("Ghostty").into_owned(),
        Place::Terminal => t!("Terminal").into_owned(),
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
