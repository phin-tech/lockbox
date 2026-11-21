//! Agents: each agent Playpen can run, beside the chosen one's image (built or not, built again on asking), the
//! providers it can use and the sandboxes it's running in. New Agent… starts from here with it chosen.

use std::collections::BTreeMap;

use playpen_core::{Docker, Kind, OpenShell, Provider};

use super::sandboxes::{agent_tile, info_card, text_value};
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

/// The Agents place's own state.
pub(super) struct AgentsPage {
    pub(super) selected: Agent,
    built: BTreeMap<&'static str, Built>,
    /// The providers OpenShell holds, once listed, or why they couldn't be.
    providers: Option<Result<Vec<Provider>, String>>,
    _listing: Option<Task<()>>,
    /// Checking or building images, while it's done.
    _working: Option<Task<()>>,
}

impl AgentsPage {
    pub(super) fn new() -> Self {
        Self {
            selected: Agent::ALL[0],
            built: BTreeMap::new(),
            providers: None,
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
}

impl MainView {
    /// Asks Docker which agents' images are built, and OpenShell for its providers, off the window's thread. A
    /// capture or a test doesn't ask.
    pub(super) fn check_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let agent = self.agents.selected;
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

    pub(super) fn select_agent(&mut self, agent: Agent, cx: &mut Context<Self>) {
        self.agents.selected = agent;
        cx.notify();
    }

    /// Chooses the agent `step` places down the list from the chosen one, or up for a negative step.
    pub(super) fn select_next_agent(&mut self, step: isize, cx: &mut Context<Self>) {
        let at = Agent::ALL
            .iter()
            .position(|agent| *agent == self.agents.selected)
            .unwrap_or(0);
        let next = at.saturating_add_signed(step).min(Agent::ALL.len() - 1);
        self.select_agent(Agent::ALL[next], cx);
    }

    /// Shows Providers with the form open for a provider of `kind`.
    fn add_provider(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.show_page(Page::Providers, window, cx);
        if let Some(providers) = &self.providers {
            providers.update(cx, |providers, cx| providers.add(kind, window, cx));
        }
    }

    /// The sandboxes running `agent`, by name.
    fn sandboxes_of(&self, agent: Agent) -> Vec<&Sandbox> {
        self.listed_sandboxes()
            .iter()
            .filter(|sandbox| sandbox.agent.as_deref() == Some(agent.id()))
            .collect()
    }

    pub(super) fn agents_heading(&self, cx: &Context<Self>) -> Div {
        let count = Agent::ALL.len();
        heading(
            t!("Agents").into_owned(),
            t!("%{count} available", count = count).into_owned(),
            cx,
        )
    }

    pub(super) fn agent_list(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let rows = Agent::ALL.map(|agent| {
            let chosen = self.agents.selected == agent;
            let running = self.sandboxes_of(agent).len();
            let said = match running {
                0 => t!("Version %{version}", version = agent.version()).into_owned(),
                count => t!(
                    "Version %{version} · %{count} sandboxes",
                    version = agent.version(),
                    count = count
                )
                .into_owned(),
            };
            let colour = match self.agents.built(agent) {
                Built::Yes => theme.success,
                Built::Building | Built::Checking => theme.info,
                Built::Failed(_) => theme.danger,
                Built::No => theme.muted_foreground,
            };
            list_row(chosen, cx)
                .id(ElementId::from((ElementId::from("agent"), agent.id())))
                .child(agent_tile(Some(agent), colour, rems(28. / 13.), cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().text_sm().font_medium().child(agent.name()))
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(said),
                        ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_agent(agent, cx);
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
        let theme = cx.theme();
        let agent = self.agents.selected;
        let built = self.agents.built(agent);
        let building = built == Built::Building;
        let hero = h_flex()
            .gap_3()
            .child(agent_tile(Some(agent), theme.primary, rems(44. / 13.), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_lg()
                            .font_semibold()
                            .child(agent.name()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(
                                t!("Version %{version}", version = agent.version()).into_owned(),
                            ),
                    ),
            );
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
                        this.start_making_for(agent, window, cx);
                    })),
            )
            .child(
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
            );
        let image = match &built {
            Built::Checking => t!("Checking…").into_owned(),
            Built::Yes => t!("Built").into_owned(),
            Built::No => t!("Not built yet: it's built when it's first needed").into_owned(),
            Built::Building => t!("Building, which takes a few minutes…").into_owned(),
            Built::Failed(_) => t!("Couldn't build it").into_owned(),
        };
        let kinds: Vec<Kind> = agent
            .provider_kinds()
            .iter()
            .filter_map(|kind| Kind::of(kind))
            .collect();
        let names: Vec<&str> = kinds.iter().map(|kind| kind.name()).collect();
        // The providers that hold a key it can use, by name, or none yet, with a way to add the first kind it takes.
        let yours = match &self.agents.providers {
            None if self.watch == Watch::Nothing => text_value(t!("Not asked").into_owned()),
            None => text_value(t!("Asking OpenShell…").into_owned()),
            Some(Err(_)) => text_value(t!("OpenShell isn't answering").into_owned()),
            Some(Ok(providers)) => {
                let usable: Vec<&str> = providers
                    .iter()
                    .filter(|provider| agent.provider_kinds().contains(&provider.kind.as_str()))
                    .map(|provider| provider.name.as_str())
                    .collect();
                match (usable.is_empty(), kinds.first().copied()) {
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
        };
        let sandboxes = self.sandboxes_of(agent);
        let mut info = vec![
            (
                t!("Image").into_owned(),
                shell::code_chip(agent.image(), cx),
            ),
            (t!("Built").into_owned(), text_value(image)),
            (t!("Can use").into_owned(), text_value(names.join(", "))),
            (t!("Your providers").into_owned(), yours),
        ];
        info.push((
            t!("Sandboxes").into_owned(),
            text_value(match sandboxes.len() {
                0 => t!("None").into_owned(),
                _ => sandboxes
                    .iter()
                    .map(|sandbox| sandbox.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            }),
        ));
        let body = v_flex()
            .gap_5()
            .p_5()
            .child(hero)
            .child(actions)
            .child(info_card(info, cx))
            .when_some(
                match &built {
                    Built::Failed(error) => Some(error.clone()),
                    _ => None,
                },
                |body, error| body.child(div().text_sm().text_color(theme.danger).child(error)),
            )
            .child(hint(
                t!(
                    "Each agent runs from an image of its own, made from its version once and kept. A sandbox reaches only its agent's model and the services attached to it."
                ),
                cx,
            ));
        div()
            .id("agent")
            .size_full()
            .overflow_y_scroll()
            .child(body)
            .into_any_element()
    }
}
