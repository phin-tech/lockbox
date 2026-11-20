//! The sandboxes on OpenShell's gateway: listed every few seconds, the chosen one shown beside the list, and
//! opening, starting, stopping and deleting it. Every call to OpenShell runs off the window's thread.

use std::path::PathBuf;
use std::time::Duration;

use lockbox_core::{Phase, open_script};

use super::*;

/// How often the sandboxes are listed again while the window's open. OpenShell sends nothing when one changes.
const LIST_EVERY: Duration = Duration::from_secs(3);

/// Something done to a sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Act {
    Start,
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
                if this.update(cx, |this, cx| this.listed(listed, cx)).is_err() {
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
            Err(error) => Listing::Failed(error.to_string()),
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
        let doing = name.clone();
        let done = cx.background_spawn(async move {
            match act {
                Act::Start => openshell.start(&doing),
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
                    if let Err(error) = done {
                        this.failed = Some(error.to_string());
                    }
                    this.refresh(window, cx);
                    cx.notify();
                });
            }
        });
        self.working.insert(name, task);
        cx.notify();
    }

    /// Opens the chosen sandbox's agent in Terminal, in the folder copied in, or a shell in a sandbox Lockbox didn't
    /// make.
    pub(super) fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(sandbox) = self.selected_sandbox().cloned() else {
            return;
        };
        if !sandbox.phase.is_ready() {
            return;
        }
        let opened = self
            .openshell()
            .map_err(anyhow::Error::from)
            .and_then(|openshell| open_in_terminal(&sandbox_script(&openshell, &sandbox)?));
        self.failed = opened.err().map(|error| {
            t!(
                "Couldn't open Terminal: %{error}",
                error = format!("{error:#}")
            )
            .into_owned()
        });
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
                .child(phase_badge(&sandbox.phase))
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
            .child(
                Button::new("open")
                    .primary()
                    .small()
                    .disabled(busy || !sandbox.phase.is_ready())
                    .label(match agent {
                        Some(agent) => t!("Open %{agent}", agent = agent.name()).into_owned(),
                        None => t!("Open").into_owned(),
                    })
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.open", window, cx)),
                    ),
            )
            .child(if sandbox.phase.can_start() {
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
                None => hint(t!("Not one Lockbox made"), cx),
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
                    "Is its gateway running? `brew services start openshell` starts it. Lockbox asks again every few seconds."
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

/// Writes the script that opens `sandbox`, as a `.command` file Terminal runs, and gives its path.
fn sandbox_script(openshell: &OpenShell, sandbox: &Sandbox) -> anyhow::Result<PathBuf> {
    use anyhow::Context as _;
    use std::os::unix::fs::PermissionsExt as _;

    let folder = std::env::temp_dir().join("lockbox");
    std::fs::create_dir_all(&folder).context("making a folder for the script")?;
    let path = folder.join(format!("{}.command", sandbox.name));
    std::fs::write(&path, open_script(openshell.binary(), sandbox))
        .context("writing the script")?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .context("making the script runnable")?;
    Ok(path)
}

/// Runs a `.command` script in a new Terminal window. `open` hands it to Terminal without asking to control it.
fn open_in_terminal(script: &std::path::Path) -> anyhow::Result<()> {
    use anyhow::Context as _;

    let status = std::process::Command::new("/usr/bin/open")
        .args(["-a", "Terminal"])
        .arg(script)
        .status()
        .context("running open")?;
    anyhow::ensure!(status.success(), "open said {status}");
    Ok(())
}
