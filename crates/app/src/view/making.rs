//! New Agent…: a sandbox made for an agent, in place of the chosen sandbox. The person names it, chooses folders to
//! copy in and the provider holding the model's key; making it runs off the window's thread, a step at a time.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::IndexPath;
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use playpen_core::{Docker, Plan, Provider, Step, suggested_name};

use super::*;

/// An agent, as the Agent menu lists it.
#[derive(Clone)]
struct AgentChoice(Agent);

impl SelectItem for AgentChoice {
    type Value = Agent;

    fn title(&self) -> SharedString {
        self.0.name().into()
    }

    fn value(&self) -> &Agent {
        &self.0
    }
}

/// A sandbox being made for an agent.
pub struct Making {
    agent: Agent,
    /// The Agent menu, searchable, since there may be more agents than fit in a row.
    agents: Entity<SelectState<SearchableVec<AgentChoice>>>,
    name: Entity<InputState>,
    /// The name Playpen last suggested, which choosing folders may change while the person hasn't typed another.
    suggested: String,
    /// The folders to copy in, in the order they were added.
    folders: Vec<PathBuf>,
    /// Every provider, once OpenShell has listed them, of which the agent can use some.
    listed: Option<Result<Vec<Provider>, String>>,
    provider: Option<String>,
    pub(super) state: MakeState,
    _changed: Subscription,
    _chose: Subscription,
    _listing: Task<()>,
    _making: Option<Task<()>>,
}

/// Where making it has got to.
pub(super) enum MakeState {
    Choosing,
    Working(Step),
    Failed(String),
}

impl MainView {
    /// Shows New Agent…'s form in place of the chosen sandbox, and asks OpenShell for its providers.
    pub(super) fn start_making(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.making.is_some() {
            return;
        }
        let agent = Agent::ClaudeCode;
        let suggested = suggested_name(agent, &[]);
        let name = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(suggested.clone());
            state.set_value(suggested.clone(), window, cx);
            state
        });
        let changed = cx.subscribe_in(&name, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.make(window, cx);
            }
        });
        let chosen = Agent::ALL.iter().position(|known| *known == agent);
        let agents = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(Agent::ALL.map(AgentChoice).to_vec()),
                chosen.map(IndexPath::new),
                window,
                cx,
            )
            .searchable(true)
        });
        let chose = cx.subscribe_in(
            &agents,
            window,
            |this, _, event: &SelectEvent<_>, window, cx| {
                let SelectEvent::Confirm(Some(agent)) = event else {
                    return;
                };
                this.choose_agent(*agent, window, cx);
            },
        );
        name.read(cx).focus_handle(cx).focus(window, cx);
        let openshell = self.openshell();
        let watching = self.watch;
        let listing = cx.spawn_in(window, async move |this, cx| {
            if watching == Watch::Nothing {
                return;
            }
            let listed = cx
                .background_spawn(
                    async move { openshell.and_then(|openshell| openshell.providers()) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.providers_listed(listed.map_err(|error| error.to_string()), cx);
            });
        });
        self.making = Some(Making {
            agent,
            agents,
            name,
            suggested,
            folders: Vec::new(),
            listed: None,
            provider: None,
            state: MakeState::Choosing,
            _changed: changed,
            _chose: chose,
            _listing: listing,
            _making: None,
        });
        self.failed = None;
        // `PLAYPEN_SHOT_FOLDERS`, separated by colons, fills the form's folders in a capture, which can't pick any.
        if cfg!(feature = "screenshot")
            && let Some(folders) = std::env::var_os("PLAYPEN_SHOT_FOLDERS")
        {
            let folders = std::env::split_paths(&folders).collect();
            self.folders_added(folders, window, cx);
        }
        cx.notify();
    }

    /// Takes in the providers OpenShell listed, and chooses the first whose key the agent can use.
    pub(super) fn providers_listed(
        &mut self,
        listed: Result<Vec<Provider>, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(making) = &mut self.making else {
            return;
        };
        making.listed = Some(listed);
        making.choose_first_provider();
        cx.notify();
    }

    /// Chooses the agent, which may change the name Playpen suggests and the providers it can use.
    pub(super) fn choose_agent(
        &mut self,
        agent: Agent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(making) = &mut self.making else {
            return;
        };
        if making.agent == agent {
            return;
        }
        making.agent = agent;
        making.suggest_name(window, cx);
        // The menu shows the agent however it was chosen, from the menu or not.
        making.agents.update(cx, |agents, cx| {
            agents.set_selected_value(&agent, window, cx);
        });
        making.choose_first_provider();
        cx.notify();
    }

    /// Closes New Agent…, giving the keys back to the list, since its name field had them.
    pub(super) fn cancel_making(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.making = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Asks for folders to copy in, as many as the person picks, and names the sandbox after them unless they've
    /// named it.
    fn choose_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some(t!("Add").into_owned().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(folders))) = chosen.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.folders_added(folders, window, cx);
            });
        })
        .detach();
    }

    /// Adds folders after those chosen already, leaving out any that's there.
    pub(super) fn folders_added(
        &mut self,
        folders: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(making) = &mut self.making else {
            return;
        };
        for folder in folders {
            if !making.folders.contains(&folder) {
                making.folders.push(folder);
            }
        }
        making.suggest_name(window, cx);
        cx.notify();
    }

    /// Leaves out a folder chosen before.
    pub(super) fn remove_folder(
        &mut self,
        folder: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(making) = &mut self.making else {
            return;
        };
        making.folders.retain(|chosen| chosen != folder);
        making.suggest_name(window, cx);
        cx.notify();
    }

    fn choose_provider(&mut self, name: String, cx: &mut Context<Self>) {
        if let Some(making) = &mut self.making {
            making.provider = Some(name);
            cx.notify();
        }
    }

    /// Makes the sandbox, off the window's thread, showing each step as it starts. Once it's made, it's chosen.
    pub(super) fn make(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let openshell = self.openshell();
        let Some(making) = &mut self.making else {
            return;
        };
        if matches!(making.state, MakeState::Working(_)) || making.provider.is_none() {
            return;
        }
        let account = match making.account() {
            Ok(account) => account,
            Err(error) => {
                making.state = MakeState::Failed(error.to_string());
                cx.notify();
                return;
            }
        };
        let plan = Plan {
            name: making.name.read(cx).value().trim().to_string(),
            agent: making.agent,
            provider: making.provider.clone(),
            account,
            folders: making.folders.clone(),
        };
        if !playpen_core::is_sandbox_name(&plan.name) {
            making.state = MakeState::Failed(playpen_core::MakeError::Name(plan.name).to_string());
            cx.notify();
            return;
        }
        let (steps, mut stepped) = mpsc::unbounded();
        let name = plan.name.clone();
        let made = cx.background_spawn(async move {
            let openshell = openshell?;
            let docker = Docker::find()?;
            plan.make(&openshell, &docker, |step| {
                let _ = steps.unbounded_send(step);
            })?;
            anyhow::Ok(())
        });
        tracing::info!(sandbox = %name, "making a sandbox");
        making.state = MakeState::Working(Step::Creating);
        making._making = Some(cx.spawn_in(window, async move |this, cx| {
            // The steps end when making does, since it holds the only sender.
            while let Some(step) = stepped.next().await {
                let _ = this.update(cx, |this, cx| {
                    if let Some(making) = &mut this.making {
                        tracing::info!(sandbox = %making.name.read(cx).value(), ?step, "making a sandbox");
                        making.state = MakeState::Working(step);
                        cx.notify();
                    }
                });
            }
            let made = made.await;
            let _ = this.update_in(cx, |this, window, cx| match made {
                Ok(()) => {
                    tracing::info!(sandbox = %name, "made a sandbox");
                    this.cancel_making(window, cx);
                    this.selected = Some(name);
                    this.refresh(window, cx);
                    cx.notify();
                }
                Err(error) => {
                    tracing::warn!(sandbox = %name, "couldn't make a sandbox: {error:#}");
                    if let Some(making) = &mut this.making {
                        making.state = MakeState::Failed(format!("{error:#}"));
                        cx.notify();
                    }
                }
            });
        }));
        cx.notify();
    }

    pub(super) fn making_panel(&self, making: &Making, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let working = matches!(making.state, MakeState::Working(_));
        let header = Self::panel_header(t!("New Agent"), cx).child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("cancel-making")
                        .ghost()
                        .xsmall()
                        .disabled(working)
                        .label(t!("Cancel").into_owned())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.cancel_making(window, cx)),
                        ),
                )
                .child(
                    Button::new("make")
                        .primary()
                        .xsmall()
                        .disabled(working || making.provider.is_none())
                        .label(t!("Make").into_owned())
                        .on_click(cx.listener(|this, _, window, cx| this.make(window, cx))),
                ),
        );
        // Each folder on a line of its own, with a button to leave it out, and Add Folders… under them.
        let folders = v_flex()
            .gap_1p5()
            .when(making.folders.is_empty(), |list| {
                list.child(hint(t!("None: it starts empty"), cx))
            })
            .children(making.folders.iter().map(|folder| {
                let shown = folder.display().to_string();
                let leaving = folder.clone();
                h_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .min_w_0()
                            .child(shell::code_chip(shown.clone(), cx)),
                    )
                    .child(
                        Button::new(ElementId::from((ElementId::from("remove-folder"), shown)))
                            .ghost()
                            .xsmall()
                            .disabled(working)
                            .icon(assets::IconName::X)
                            .tooltip(t!("Leave Out").into_owned())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_folder(&leaving, window, cx);
                            })),
                    )
            }))
            .child(
                h_flex().child(
                    Button::new("choose-folders")
                        .xsmall()
                        .disabled(working)
                        .icon(assets::IconName::FolderPlus)
                        .label(t!("Add Folders…").into_owned())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.choose_folders(window, cx)),
                        ),
                ),
            );
        let providers = match making.usable() {
            None => hint(t!("Asking OpenShell…"), cx),
            Some(Err(error)) => div()
                .text_sm()
                .text_color(theme.danger)
                .child(error.clone()),
            Some(Ok(providers)) if providers.is_empty() => hint(
                t!(
                    "OpenShell has no provider with a key for %{agent}. Add one in Settings ▸ Providers, then open New Agent… again.",
                    agent = making.agent.name()
                ),
                cx,
            ),
            Some(Ok(providers)) => v_flex().gap_1().children(providers.iter().map(|provider| {
                let name = provider.name.clone();
                Radio::new(ElementId::from((ElementId::from("provider"), name.clone())))
                    .label(format!(
                        "{} ({})",
                        provider.name,
                        provider
                            .playpen_kind()
                            .map_or(provider.kind.as_str(), |kind| kind.name())
                    ))
                    .checked(making.provider.as_ref() == Some(&provider.name))
                    .disabled(working)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.choose_provider(name.clone(), cx)),
                    )
            })),
        };
        let state = match &making.state {
            MakeState::Choosing => hint(
                t!(
                    "The folders are copied in, leaving out what each .gitignore does, so the agent works on copies, side by side in /sandbox, where it starts. The provider's key never enters the sandbox: OpenShell puts it on the agent's requests to its model, and the agent can reach nothing else."
                ),
                cx,
            ),
            MakeState::Working(step) => h_flex().gap_2().child(div().child(match step {
                Step::Building => t!(
                    "Building the %{agent} image. This happens once for each version, and takes a few minutes…",
                    agent = making.agent.name()
                ),
                Step::Creating => t!("Making the sandbox…"),
                Step::Copying => t!("Copying the folders in…"),
                Step::Preparing => t!("Setting up %{agent}…", agent = making.agent.name()),
            }
            .into_owned())),
            MakeState::Failed(error) => div().text_sm().text_color(theme.danger).child(error.clone()),
        };
        let body = v_flex()
            .gap_5()
            .p_4()
            .child(
                section(t!("Agent"), cx).child(
                    div().w(rems(20.)).child(
                        Select::new(&making.agents)
                            .small()
                            .search_placeholder(t!("Search").into_owned())
                            .disabled(working),
                    ),
                ),
            )
            .child(
                section(t!("Name"), cx).child(div().w(rems(20.)).child(small_field(&making.name))),
            )
            .child(section(t!("Folders"), cx).child(folders))
            .child(section(t!("Model key"), cx).child(providers))
            .child(state);
        v_flex()
            .size_full()
            .child(header)
            .child(div().id("making").flex_1().overflow_y_scroll().child(body))
            .into_any_element()
    }
}

impl Making {
    /// Names the sandbox after the agent and the folders, unless the person has typed a name of their own.
    fn suggest_name(&mut self, window: &mut Window, cx: &mut App) {
        let typed = self.name.read(cx).value().trim().to_string();
        if typed == self.suggested || typed.is_empty() {
            let suggested = suggested_name(self.agent, &self.folders);
            self.name
                .update(cx, |name, cx| name.set_value(suggested.clone(), window, cx));
            self.suggested = suggested;
        }
    }

    /// The providers whose key the agent can use, once OpenShell has listed them.
    fn usable(&self) -> Option<Result<Vec<&Provider>, &String>> {
        let kinds = self.agent.provider_kinds();
        self.listed.as_ref().map(|listed| {
            listed.as_ref().map(|providers| {
                providers
                    .iter()
                    .filter(|provider| kinds.contains(&provider.kind.as_str()))
                    .collect()
            })
        })
    }

    fn choose_first_provider(&mut self) {
        self.provider = self
            .usable()
            .and_then(Result::ok)
            .and_then(|providers| providers.first().map(|provider| provider.name.clone()));
    }

    /// The ChatGPT account Playpen kept for the chosen provider, when it's a ChatGPT subscription.
    fn account(&self) -> anyhow::Result<Option<playpen_core::Account>> {
        let chosen = self.usable().and_then(Result::ok).and_then(|providers| {
            providers
                .into_iter()
                .find(|provider| Some(&provider.name) == self.provider.as_ref())
                .cloned()
        });
        let Some(provider) = chosen else {
            return Ok(None);
        };
        if provider.playpen_kind() != Some(playpen_core::Kind::ChatGptSubscription) {
            return Ok(None);
        }
        let folder = crate::settings::file("chatgpt")
            .context("Playpen keeps no files here, so it has no ChatGPT account")?;
        playpen_core::Account::load(&folder, &provider.name)
            .map(Some)
            .context("Sign the ChatGPT provider in again in Settings ▸ Providers")
    }
}

#[cfg(test)]
impl Making {
    pub(super) fn chosen_provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    pub(super) fn name_typed(&self, cx: &App) -> String {
        self.name.read(cx).value().to_string()
    }
}
