//! Agents: each agent Playpen can run, and the person's own, beside the chosen one's image (built or not, built again
//! on asking, or the person's own), the providers it can use and the sandboxes it's running in. New Agent… starts from
//! here with it chosen.

use std::collections::BTreeMap;
use std::path::PathBuf;

use playpen_core::{CustomAgent, CustomAgents, Docker, Harness, Kind, OpenShell, Provider};

use super::sandboxes::{Mark, agent_tile, info_card, text_value};
use super::*;

/// Whether an agent's image is on this Mac.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Built {
    Checking,
    Yes,
    No,
    Building,
    Failed(String),
}

/// The agent chosen in the list: one of Playpen's, or one of the person's own by its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Chosen {
    Agent(Agent),
    Custom(String),
}

/// An agent of the person's own being described, new or changed.
pub(super) struct CustomForm {
    /// The agent being changed, by the name it had; none for a new one.
    editing: Option<String>,
    name: Entity<InputState>,
    image: Entity<InputState>,
    command: Entity<InputState>,
    continue_command: Entity<InputState>,
    program: Entity<InputState>,
    takes: Vec<Kind>,
    problem: Option<String>,
}

/// The Agents place's own state.
pub(super) struct AgentsPage {
    pub(super) selected: Chosen,
    built: BTreeMap<&'static str, Built>,
    /// The providers OpenShell holds, once listed, or why they couldn't be.
    providers: Option<Result<Vec<Provider>, String>>,
    /// The person's own agents, and where they're kept: nowhere in a capture or a test.
    pub(super) custom: CustomAgents,
    path: Option<PathBuf>,
    /// Why the file of the person's own agents couldn't be read or written.
    failed: Option<String>,
    /// An agent of the person's own being described, in the detail panel's place.
    pub(super) form: Option<CustomForm>,
    /// The person's own image for the chosen one of Playpen's agents, as typed.
    image: Option<Entity<InputState>>,
    _listing: Option<Task<()>>,
    /// Checking or building images, while it's done.
    _working: Option<Task<()>>,
}

impl AgentsPage {
    pub(super) fn new(watch: Watch) -> Self {
        let path = (watch == Watch::OpenShell)
            .then(|| crate::settings::file("agents.toml"))
            .flatten();
        let (custom, failed) = match &path {
            Some(path) => match CustomAgents::load(path) {
                Ok(custom) => (custom, None),
                Err(error) => (CustomAgents::default(), Some(error)),
            },
            None => (CustomAgents::default(), None),
        };
        Self {
            selected: Chosen::Agent(Agent::ALL[0]),
            built: BTreeMap::new(),
            providers: None,
            custom,
            path,
            failed,
            form: None,
            image: None,
            _listing: None,
            _working: None,
        }
    }

    fn built(&self, agent: Agent) -> Built {
        self.built
            .get(agent.id())
            .cloned()
            .unwrap_or(Built::Checking)
    }

    /// Every agent in the list's order: Playpen's, then the person's own.
    fn every(&self) -> Vec<Chosen> {
        Agent::ALL
            .into_iter()
            .map(Chosen::Agent)
            .chain(
                self.custom
                    .agents
                    .iter()
                    .map(|custom| Chosen::Custom(custom.name.clone())),
            )
            .collect()
    }

    pub(super) fn count(&self) -> usize {
        Agent::ALL.len() + self.custom.agents.len()
    }

    fn save(&mut self) {
        if let Some(path) = &self.path {
            self.failed = self.custom.save(path).err();
        }
    }
}

impl MainView {
    /// Every agent a sandbox can be made for: Playpen's, then the person's own.
    pub(super) fn harnesses(&self) -> Vec<Harness> {
        Agent::ALL
            .into_iter()
            .map(Harness::from)
            .chain(
                self.agents
                    .custom
                    .agents
                    .iter()
                    .cloned()
                    .map(Harness::Custom),
            )
            .collect()
    }

    /// The agent chosen in Agents, as New Agent… makes a sandbox for it.
    pub(super) fn selected_harness(&self) -> Harness {
        match &self.agents.selected {
            Chosen::Agent(agent) => Harness::Agent(*agent),
            Chosen::Custom(name) => self
                .agents
                .custom
                .find(name)
                .cloned()
                .map_or(Agent::ALL[0].into(), Harness::Custom),
        }
    }

    /// Asks Docker which agents' images are built, and OpenShell for its providers, off the window's thread. A
    /// capture or a test doesn't ask.
    pub(super) fn check_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_image_field(window, cx);
        if self.watch == Watch::Nothing {
            return;
        }
        self.agents._listing = Some(cx.spawn_in(window, async move |this, cx| {
            let listed = cx
                .background_spawn(async move {
                    OpenShell::find().and_then(|openshell| openshell.providers())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.agents.providers = Some(listed.map_err(|error| error.to_string()));
                cx.notify();
            });
        }));
        let task = cx.spawn_in(window, async move |this, cx| {
            let built = cx
                .background_spawn(async move {
                    let docker = Docker::find();
                    Agent::ALL.map(|agent| {
                        let built = match &docker {
                            Ok(docker) => match docker.has_image(&agent.image()) {
                                Ok(true) => Built::Yes,
                                Ok(false) => Built::No,
                                Err(error) => Built::Failed(error.to_string()),
                            },
                            Err(error) => Built::Failed(error.to_string()),
                        };
                        (agent.id(), built)
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                for (id, built) in built {
                    // A build under way says so until it's done.
                    if this.agents.built.get(id) != Some(&Built::Building) {
                        this.agents.built.insert(id, built);
                    }
                }
                cx.notify();
            });
        });
        self.agents._working = Some(task);
    }

    /// Builds the chosen agent's image, or builds it again, off the window's thread.
    pub(super) fn build_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Chosen::Agent(agent) = self.agents.selected else {
            return;
        };
        if self.agents.built(agent) == Built::Building {
            return;
        }
        self.agents.built.insert(agent.id(), Built::Building);
        tracing::info!(agent = agent.id(), "building an image");
        let task = cx.spawn_in(window, async move |this, cx| {
            let built = cx
                .background_spawn(async move {
                    Docker::find().and_then(|docker| agent.build_image(&docker))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let built = match built {
                    Ok(()) => Built::Yes,
                    Err(error) => {
                        tracing::warn!(agent = agent.id(), "couldn't build its image: {error}");
                        Built::Failed(error.to_string())
                    }
                };
                this.agents.built.insert(agent.id(), built);
                cx.notify();
            });
        });
        self.agents._working = Some(task);
        cx.notify();
    }

    pub(super) fn select_agent(
        &mut self,
        chosen: Chosen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agents.selected = chosen;
        self.agents.form = None;
        self.show_image_field(window, cx);
        cx.notify();
    }

    /// Fills the field for the person's own image with the one the chosen agent starts from, if it's one of Playpen's.
    fn show_image_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Chosen::Agent(agent) = self.agents.selected else {
            self.agents.image = None;
            return;
        };
        let theirs = crate::settings::current(cx)
            .agent_image(agent)
            .unwrap_or_default();
        self.agents.image = Some(cx.new(|cx| {
            let mut field = InputState::new(window, cx).placeholder("ghcr.io/you/agent:tag");
            field.set_value(theirs, window, cx);
            field
        }));
    }

    /// Starts the chosen one of Playpen's agents from the image typed, or from Playpen's own with `Theirs::No`.
    fn use_own_image(&mut self, theirs: Theirs, window: &mut Window, cx: &mut Context<Self>) {
        let Chosen::Agent(agent) = self.agents.selected else {
            return;
        };
        let image = match (theirs, &self.agents.image) {
            (Theirs::Yes, Some(field)) => Some(field.read(cx).value().to_string()),
            _ => None,
        };
        let said = if image
            .as_deref()
            .is_some_and(|image| !image.trim().is_empty())
        {
            t!("%{agent} starts from your image", agent = agent.name())
        } else {
            t!("%{agent} starts from Playpen's image", agent = agent.name())
        };
        crate::settings::set_agent_image(agent, image, cx);
        self.show_image_field(window, cx);
        Toast::new(said.into_owned())
            .key("agent-image")
            .show(window, cx);
        cx.notify();
    }

    /// Chooses the agent `step` places down the list from the chosen one, or up for a negative step.
    pub(super) fn select_next_agent(
        &mut self,
        step: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let every = self.agents.every();
        let at = every
            .iter()
            .position(|chosen| *chosen == self.agents.selected)
            .unwrap_or(0);
        let next = at.saturating_add_signed(step).min(every.len() - 1);
        self.select_agent(every[next].clone(), window, cx);
    }

    /// Shows Providers with the form open for a provider of `kind`.
    fn add_provider(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.show_page(Page::Providers, window, cx);
        if let Some(providers) = &self.providers {
            providers.update(cx, |providers, cx| providers.add(kind, window, cx));
        }
    }

    /// The sandboxes running `chosen`, by name.
    fn sandboxes_running(&self, chosen: &Chosen) -> Vec<&Sandbox> {
        self.listed_sandboxes()
            .iter()
            .filter(|sandbox| match (chosen, Mark::of(sandbox)) {
                (Chosen::Agent(agent), Mark::Agent(running)) => *agent == running,
                (Chosen::Custom(name), Mark::Custom(running)) => *name == running,
                _ => false,
            })
            .collect()
    }

    pub(super) fn agents_heading(&self, cx: &Context<Self>) -> Div {
        heading(
            t!("Agents").into_owned(),
            t!("%{count} available", count = self.agents.count()).into_owned(),
            cx,
        )
        .child(
            Button::new("new-custom-agent")
                .ghost()
                .small()
                .rounded(px(999.))
                .icon(assets::IconName::Plus)
                .tooltip(t!("New Custom Agent…").into_owned())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.run("agent.new-custom", window, cx);
                })),
        )
    }

    pub(super) fn agent_list(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let rows = self.agents.every().into_iter().map(|chosen| {
            let picked = self.agents.selected == chosen;
            let running = self.sandboxes_running(&chosen).len();
            let (mark, name, said, colour) = match &chosen {
                Chosen::Agent(agent) => {
                    let version = agent.version();
                    let said = match running {
                        0 => t!("Version %{version}", version = version).into_owned(),
                        1 => t!("Version %{version} · 1 sandbox", version = version).into_owned(),
                        count => t!(
                            "Version %{version} · %{count} sandboxes",
                            version = version,
                            count = count
                        )
                        .into_owned(),
                    };
                    let colour = match self.agents.built(*agent) {
                        Built::Yes => theme.success,
                        Built::Building | Built::Checking => theme.info,
                        Built::Failed(_) => theme.danger,
                        Built::No => theme.muted_foreground,
                    };
                    (Mark::Agent(*agent), agent.name().to_string(), said, colour)
                }
                Chosen::Custom(name) => {
                    let said = match running {
                        0 => t!("Yours").into_owned(),
                        1 => t!("Yours · 1 sandbox").into_owned(),
                        count => t!("Yours · %{count} sandboxes", count = count).into_owned(),
                    };
                    (
                        Mark::Custom(name.clone()),
                        name.clone(),
                        said,
                        theme.primary,
                    )
                }
            };
            let id = match &chosen {
                Chosen::Agent(agent) => ElementId::from((ElementId::from("agent"), agent.id())),
                Chosen::Custom(name) => {
                    ElementId::from((ElementId::from("custom-agent"), name.clone()))
                }
            };
            list_row(picked, cx)
                .id(id)
                .child(agent_tile(&mark, colour, rems(28. / 13.), cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().text_sm().font_medium().child(name))
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(said),
                        ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_agent(chosen.clone(), window, cx);
                    this.focus.focus(window, cx);
                }))
        });
        v_flex().size_full().child(
            v_flex()
                .id("agents")
                .flex_1()
                .overflow_y_scroll()
                .px_2()
                .pt_1()
                .gap(px(2.))
                .children(rows),
        )
    }

    pub(super) fn agent_detail(&self, cx: &Context<Self>) -> AnyElement {
        if let Some(form) = &self.agents.form {
            return self.custom_form_panel(form, cx);
        }
        match &self.agents.selected {
            Chosen::Agent(agent) => self.own_agent_detail(*agent, cx),
            Chosen::Custom(name) => match self.agents.custom.find(name) {
                Some(custom) => self.custom_agent_detail(custom, cx),
                None => div().into_any_element(),
            },
        }
    }

    /// The providers that hold a key it can use, by name, or none yet, with a way to add the first kind it takes.
    fn your_providers(
        &self,
        takes: impl Fn(&str) -> bool,
        first: Option<Kind>,
        cx: &Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        match &self.agents.providers {
            None if self.watch == Watch::Nothing => text_value(t!("Not asked").into_owned()),
            None => text_value(t!("Asking OpenShell…").into_owned()),
            Some(Err(_)) => text_value(t!("OpenShell isn't answering").into_owned()),
            Some(Ok(providers)) => {
                let usable: Vec<&str> = providers
                    .iter()
                    .filter(|provider| takes(&provider.kind))
                    .map(|provider| provider.name.as_str())
                    .collect();
                match (usable.is_empty(), first) {
                    (true, Some(first)) => h_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_color(theme.warning)
                                .child(t!("None yet").into_owned()),
                        )
                        .child(
                            Button::new("add-provider-for")
                                .xsmall()
                                .icon(assets::IconName::Plus)
                                .label(t!("Add %{kind}…", kind = first.name()).into_owned())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.add_provider(first, window, cx);
                                })),
                        ),
                    (true, None) => text_value(t!("None yet").into_owned()),
                    (false, _) => text_value(usable.join(", ")),
                }
            }
        }
    }

    /// Buttons that jump to each sandbox running it, or None.
    fn sandbox_buttons(&self, chosen: &Chosen, cx: &Context<Self>) -> Div {
        let sandboxes = self.sandboxes_running(chosen);
        if sandboxes.is_empty() {
            return div()
                .text_color(cx.theme().muted_foreground)
                .child(t!("None").into_owned());
        }
        let gateway_key = self.gateway_key();
        h_flex()
            .flex_wrap()
            .justify_end()
            .gap_1p5()
            .children(sandboxes.into_iter().map(|sandbox| {
                let target = SandboxKey {
                    gateway: gateway_key.clone(),
                    name: sandbox.name.clone(),
                };
                Button::new(ElementId::from((
                    ElementId::from("jump-sandbox"),
                    sandbox.name.clone(),
                )))
                .outline()
                .xsmall()
                .icon(assets::IconName::Box)
                .label(sandbox.name.clone())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.show_page(Page::Sandboxes, window, cx);
                    this.select(target.clone(), cx);
                }))
            }))
    }

    /// An agent's mark, name, a badge and a line under it, over its buttons, pinned above what scrolls.
    fn agent_header(
        &self,
        mark: &Mark,
        badge: Option<Tag>,
        said: String,
        actions: Div,
        cx: &Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let hero = h_flex()
            .gap_3()
            .items_center()
            .child(agent_tile(mark, theme.primary, rems(42. / 13.), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_lg()
                                    .font_semibold()
                                    .child(mark.name()),
                            )
                            .when_some(badge, |row, badge| row.child(badge)),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(said),
                    ),
            );
        v_flex()
            .flex_none()
            .gap_3()
            .px_5()
            .py_4()
            .border_b_1()
            .border_color(shell::hairline(theme))
            .child(hero)
            .child(actions)
    }

    fn own_agent_detail(&self, agent: Agent, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let theirs = crate::settings::current(cx).agent_image(agent);
        let built = self.agents.built(agent);
        let building = built == Built::Building;
        let build_badge = match (&theirs, &built) {
            (Some(_), _) => Some(
                shell::badge(Tag::secondary())
                    .xsmall()
                    .child(t!("Your image").into_owned()),
            ),
            (None, Built::Yes) => Some(
                shell::badge(Tag::success())
                    .xsmall()
                    .child(t!("Built").into_owned()),
            ),
            (None, Built::No) => Some(
                shell::badge(Tag::secondary())
                    .xsmall()
                    .child(t!("Not built").into_owned()),
            ),
            (None, Built::Building) => Some(
                shell::badge(Tag::info())
                    .xsmall()
                    .child(t!("Building…").into_owned()),
            ),
            (None, Built::Failed(_)) => Some(
                shell::badge(Tag::danger())
                    .xsmall()
                    .child(t!("Build failed").into_owned()),
            ),
            (None, Built::Checking) => None,
        };
        let chosen = Chosen::Agent(agent);
        let actions = h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                Button::new("new-agent-of")
                    .primary()
                    .small()
                    .icon(assets::IconName::Plus)
                    .label(t!("New %{agent} Sandbox…", agent = agent.name()).into_owned())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_making_for(agent.into(), window, cx);
                    })),
            )
            .when(theirs.is_none(), |actions| {
                actions.child(
                    Button::new("build-image")
                        .small()
                        .disabled(building || self.watch == Watch::Nothing)
                        .loading(building)
                        .icon(assets::IconName::Hammer)
                        .label(match built {
                            Built::Yes => t!("Build Again").into_owned(),
                            _ => t!("Build Image").into_owned(),
                        })
                        .on_click(cx.listener(|this, _, window, cx| this.build_image(window, cx))),
                )
            });
        let header = self.agent_header(
            &Mark::Agent(agent),
            build_badge,
            t!("Version %{version}", version = agent.version()).into_owned(),
            actions,
            cx,
        );
        let image = match (&theirs, &built) {
            (Some(_), _) => t!("Not built: the gateway pulls your image").into_owned(),
            (None, Built::Checking) => t!("Checking…").into_owned(),
            (None, Built::Yes) => t!("Built").into_owned(),
            (None, Built::No) => {
                t!("Not built yet: it's built when it's first needed").into_owned()
            }
            (None, Built::Building) => t!("Building, which takes a few minutes…").into_owned(),
            (None, Built::Failed(_)) => t!("Couldn't build it").into_owned(),
        };
        let kinds: Vec<Kind> = agent.model_kinds().to_vec();
        let names: Vec<&str> = kinds.iter().map(|kind| kind.name()).collect();
        let info = vec![
            (
                t!("Image").into_owned(),
                shell::code_chip(theirs.clone().unwrap_or_else(|| agent.image()), cx),
            ),
            (t!("Built").into_owned(), text_value(image)),
            (t!("Can use").into_owned(), text_value(names.join(", "))),
            (
                t!("Your providers").into_owned(),
                self.your_providers(|kind| agent.takes(kind), kinds.first().copied(), cx),
            ),
            (
                t!("Sandboxes").into_owned(),
                self.sandbox_buttons(&chosen, cx),
            ),
        ];
        let own_image = self.agents.image.as_ref().map(|field| {
            section(t!("Your own image"), cx)
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().w(rems(20.)).child(small_field(field)))
                        .child(
                            Button::new("use-own-image")
                                .small()
                                .label(t!("Use This Image").into_owned())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.use_own_image(Theirs::Yes, window, cx);
                                })),
                        )
                        .when(theirs.is_some(), |row| {
                            row.child(
                                Button::new("use-playpens-image")
                                    .ghost()
                                    .small()
                                    .label(t!("Use Playpen's").into_owned())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.use_own_image(Theirs::No, window, cx);
                                    })),
                            )
                        }),
                )
                .child(hint(
                    t!(
                        "Every gateway pulls it, and nothing is built. It needs %{program}, the program the policy lets reach the model, and node, which writes the agent's settings.",
                        program = agent.program()
                    ),
                    cx,
                ))
        });
        let body = v_flex()
            .gap_5()
            .p_5()
            .child(info_card(info, cx))
            .when_some(
                match &built {
                    Built::Failed(error) if theirs.is_none() => Some(error.clone()),
                    _ => None,
                },
                |body, error| body.child(div().text_sm().text_color(theme.danger).child(error)),
            )
            .when_some(own_image, |body, own_image| body.child(own_image))
            .child(hint(
                t!(
                    "Each agent runs from an image of its own, made from its version once and kept. A sandbox reaches only its agent's model and the services attached to it."
                ),
                cx,
            ));
        v_flex()
            .size_full()
            .child(header)
            .child(div().id("agent").flex_1().overflow_y_scroll().child(body))
            .into_any_element()
    }

    fn custom_agent_detail(&self, custom: &CustomAgent, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let chosen = Chosen::Custom(custom.name.clone());
        let harness = Harness::Custom(custom.clone());
        let editing = custom.name.clone();
        let deleting = custom.name.clone();
        let actions = h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                Button::new("new-agent-of")
                    .primary()
                    .small()
                    .icon(assets::IconName::Plus)
                    .label(t!("New %{agent} Sandbox…", agent = custom.name.clone()).into_owned())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_making_for(harness.clone(), window, cx);
                    })),
            )
            .child(
                Button::new("edit-custom-agent")
                    .small()
                    .icon(assets::IconName::Pencil)
                    .label(t!("Edit…").into_owned())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.edit_custom_agent(Some(editing.clone()), window, cx);
                    })),
            )
            .child(
                Button::new("delete-custom-agent")
                    .ghost()
                    .small()
                    .icon(assets::IconName::Trash)
                    .label(t!("Delete").into_owned())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.delete_custom_agent(&deleting, window, cx);
                    })),
            );
        let header = self.agent_header(
            &Mark::Custom(custom.name.clone()),
            Some(
                shell::badge(Tag::secondary())
                    .xsmall()
                    .child(t!("Yours").into_owned()),
            ),
            custom.image.clone(),
            actions,
            cx,
        );
        let reads: Vec<String> = custom
            .takes
            .iter()
            .map(|kind| format!("{} ({})", kind.name(), kind.variable()))
            .collect();
        let continues = custom
            .continue_command
            .clone()
            .unwrap_or_else(|| t!("It can't").into_owned());
        let takes = custom.clone();
        let info = vec![
            (
                t!("Image").into_owned(),
                shell::code_chip(custom.image.clone(), cx),
            ),
            (
                t!("Starts with").into_owned(),
                shell::code_chip(custom.command.clone(), cx),
            ),
            (t!("Continues with").into_owned(), text_value(continues)),
            (
                t!("Reaches its model from").into_owned(),
                shell::code_chip(custom.program.clone(), cx),
            ),
            (t!("Reads").into_owned(), text_value(reads.join(", "))),
            (
                t!("Your providers").into_owned(),
                self.your_providers(
                    move |kind| takes.takes(kind),
                    custom.takes.first().copied(),
                    cx,
                ),
            ),
            (
                t!("Sandboxes").into_owned(),
                self.sandbox_buttons(&chosen, cx),
            ),
        ];
        let body = v_flex()
            .gap_5()
            .p_5()
            .child(info_card(info, cx))
            .when_some(self.agents.failed.clone(), |body, error| {
                body.child(div().text_sm().text_color(theme.danger).child(error))
            })
            .child(hint(
                t!(
                    "A sandbox for it reaches only the host its model key is for, from the program named, and the services attached to it. Changing it here leaves its sandboxes as they were made."
                ),
                cx,
            ));
        v_flex()
            .size_full()
            .child(header)
            .child(
                div()
                    .id("custom-agent")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(body),
            )
            .into_any_element()
    }

    /// Shows the form for a new agent of the person's own, or for changing the one called `name`, over Agents.
    pub(super) fn edit_custom_agent(
        &mut self,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = name
            .as_deref()
            .and_then(|name| self.agents.custom.find(name))
            .cloned();
        let start = existing.clone().unwrap_or_else(|| CustomAgent {
            name: self.agents.custom.unused_name("agent"),
            takes: vec![Kind::OpenRouter],
            ..CustomAgent::default()
        });
        let field =
            |value: &str, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
                let value = value.to_string();
                let placeholder = placeholder.to_string();
                cx.new(|cx| {
                    let mut state = InputState::new(window, cx).placeholder(placeholder);
                    state.set_value(value, window, cx);
                    state
                })
            };
        let name_field = field(&start.name, "aider", window, cx);
        let form = CustomForm {
            editing: existing.map(|existing| existing.name),
            image: field(&start.image, "ghcr.io/you/aider:1.0", window, cx),
            command: field(&start.command, "aider", window, cx),
            continue_command: field(
                start.continue_command.as_deref().unwrap_or_default(),
                "aider --restore-chat-history",
                window,
                cx,
            ),
            program: field(&start.program, "/usr/local/bin/python3.12", window, cx),
            name: name_field.clone(),
            takes: start.takes,
            problem: None,
        };
        self.page = Page::Agents;
        self.agents.form = Some(form);
        name_field.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn toggle_custom_kind(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.agents.form {
            if let Some(at) = form.takes.iter().position(|taken| *taken == kind) {
                form.takes.remove(at);
            } else {
                form.takes.push(kind);
            }
            cx.notify();
        }
    }

    pub(super) fn cancel_custom_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agents.form = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Saves the agent described, if it can be used as it is, and chooses it.
    fn save_custom_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.agents.form else {
            return;
        };
        let text = |field: &Entity<InputState>| field.read(cx).value().trim().to_string();
        let continuing = text(&form.continue_command);
        let agent = CustomAgent {
            name: text(&form.name),
            image: text(&form.image),
            command: text(&form.command),
            continue_command: (!continuing.is_empty()).then_some(continuing),
            program: text(&form.program),
            takes: CustomAgent::KINDS
                .into_iter()
                .filter(|kind| form.takes.contains(kind))
                .collect(),
        };
        let editing = form.editing.clone();
        let problem = match agent.check() {
            Err(error) => Some(error.to_string()),
            Ok(())
                if editing.as_deref() != Some(&agent.name)
                    && self.agents.custom.find(&agent.name).is_some() =>
            {
                Some(
                    t!(
                        "There's already an agent called “%{name}”.",
                        name = agent.name.clone()
                    )
                    .into_owned(),
                )
            }
            Ok(()) => None,
        };
        if let Some(problem) = problem {
            if let Some(form) = &mut self.agents.form {
                form.problem = Some(problem);
            }
            cx.notify();
            return;
        }
        let name = agent.name.clone();
        let custom = &mut self.agents.custom.agents;
        match editing.and_then(|editing| custom.iter().position(|known| known.name == editing)) {
            Some(at) => custom[at] = agent,
            None => custom.push(agent),
        }
        self.agents.save();
        tracing::info!(agent = %name, "saved a custom agent");
        self.agents.form = None;
        self.select_agent(Chosen::Custom(name), window, cx);
        self.focus.focus(window, cx);
    }

    fn delete_custom_agent(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.agents.custom.agents.retain(|agent| agent.name != name);
        self.agents.save();
        Toast::new(
            t!(
                "Deleted “%{name}”; its sandboxes are as they were",
                name = name
            )
            .into_owned(),
        )
        .key("custom-agent-deleted")
        .show(window, cx);
        self.select_agent(Chosen::Agent(Agent::ALL[0]), window, cx);
    }

    fn custom_form_panel(&self, form: &CustomForm, cx: &Context<Self>) -> AnyElement {
        let field = |title, state: &Entity<InputState>, cx: &Context<Self>| {
            section(title, cx).child(div().w(rems(24.)).child(small_field(state)))
        };
        let kinds = h_flex()
            .flex_wrap()
            .gap_2()
            .children(CustomAgent::KINDS.map(|kind| {
                let picked = form.takes.contains(&kind);
                choice_chip(picked, cx)
                    .id(ElementId::from((
                        ElementId::from("custom-kind"),
                        kind.name(),
                    )))
                    .items_center()
                    .child(
                        Icon::new(if picked {
                            assets::IconName::Check
                        } else {
                            assets::IconName::Plus
                        })
                        .xsmall(),
                    )
                    .child(format!("{} · {}", kind.name(), kind.variable()))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_custom_kind(kind, cx)))
            }));
        let body = v_flex()
            .gap_5()
            .p_4()
            .child(field(t!("Name"), &form.name, cx))
            .child(field(t!("Image"), &form.image, cx))
            .child(field(t!("Starts with"), &form.command, cx))
            .child(field(t!("Continues with"), &form.continue_command, cx))
            .child(
                field(t!("Reaches its model from"), &form.program, cx).child(hint(
                    t!(
                        "The program's full path in the image, links resolved: the policy lets only it out, to the host the model key is for. For a Python harness, Python itself."
                    ),
                    cx,
                )),
            )
            .child(section(t!("Reads"), cx).child(kinds).child(hint(
                t!(
                    "Each key is in the variable beside it, as a stand-in that OpenShell swaps for the real key on its way out."
                ),
                cx,
            )));
        let footer_left = match &form.problem {
            Some(problem) => div()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(problem.clone()),
            None => div(),
        };
        let cancel = Button::new("cancel-custom-agent")
            .ghost()
            .small()
            .label(t!("Cancel").into_owned())
            .on_click(cx.listener(|this, _, window, cx| this.cancel_custom_form(window, cx)));
        let primary = Button::new("save-custom-agent")
            .primary()
            .small()
            .label(match form.editing {
                Some(_) => t!("Save").into_owned(),
                None => t!("Add Agent").into_owned(),
            })
            .on_click(cx.listener(|this, _, window, cx| this.save_custom_agent(window, cx)));
        self.form_panel(
            match form.editing {
                Some(_) => t!("Edit Custom Agent"),
                None => t!("New Custom Agent"),
            },
            "custom-agent-form",
            body,
            footer_left,
            cancel,
            primary,
            false,
            |this, _, window, cx| this.cancel_custom_form(window, cx),
            cx,
        )
    }
}

/// Whether one of Playpen's agents starts from the person's image or from Playpen's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Theirs {
    Yes,
    No,
}

#[cfg(test)]
impl MainView {
    /// Types into the custom agent form: its name, image, start, continue and program, in that order.
    pub(super) fn fill_custom_agent(
        &mut self,
        values: [&str; 5],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &self.agents.form else {
            return;
        };
        let fields = [
            form.name.clone(),
            form.image.clone(),
            form.command.clone(),
            form.continue_command.clone(),
            form.program.clone(),
        ];
        for (field, value) in fields.iter().zip(values) {
            field.update(cx, |field, cx| {
                field.set_value(value.to_string(), window, cx)
            });
        }
    }

    pub(super) fn run_save_custom_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_custom_agent(window, cx);
    }
}
