//! New Agent…: a sandbox made for an agent, in place of the chosen sandbox. The person names it, chooses folders to
//! copy in and the provider holding the model's key; making it runs off the window's thread, a step at a time.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::IndexPath;
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use std::collections::BTreeSet;

use gpui_kit::component::checkbox::Checkbox;
use playpen_core::{
    Docker, Harness, Kit, Model, Plan, Profile, Provider, Service, SetupFolder, Step, Variable,
    VariableValue, suggested_name,
};

use super::*;

/// An agent, as the Agent menu lists it.
#[derive(Clone)]
struct AgentChoice(Harness);

impl SelectItem for AgentChoice {
    type Value = Harness;

    fn title(&self) -> SharedString {
        self.0.name().to_string().into()
    }

    fn value(&self) -> &Harness {
        &self.0
    }
}

/// A sandbox being made for an agent.
pub struct Making {
    /// The gateway the sandbox is made on, by its name: its providers are the ones offered.
    gateway: String,
    agent: Harness,
    /// The Agent menu, searchable, since there may be more agents than fit in a row.
    agents: Entity<SelectState<SearchableVec<AgentChoice>>>,
    name: Entity<InputState>,
    /// The model to start with, for an agent that reaches whichever service its key is for; empty for its own choice.
    model: Entity<InputState>,
    /// The name Playpen last suggested, which choosing folders may change while the person hasn't typed another.
    suggested: String,
    /// The folders to copy in, in the order they were added.
    folders: Vec<PathBuf>,
    /// Every provider, once OpenShell has listed them, of which the agent can use some.
    listed: Option<Result<Vec<Provider>, String>>,
    provider: Option<String>,
    /// The provider profiles on the gateway, once listed, which say which providers can be services.
    profiles: Option<Vec<Profile>>,
    /// The services chosen, by provider name.
    services: BTreeSet<String>,
    /// The kits chosen, in the order they were, which give the sandbox their services and variables, and the first's
    /// model key the agent can use.
    kits: Vec<String>,
    /// The chosen kits' variables, a later kit's winning where two set one, with the names that were set twice.
    variables: Vec<Variable>,
    /// The chosen kits' setup folders, in the order the kits were chosen.
    setups: Vec<SetupFolder>,
    overridden: Vec<String>,
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
        // From Agents, for the agent chosen there.
        let agent = match self.page {
            Page::Agents => self.selected_harness(),
            Page::Sandboxes | Page::Kits | Page::Providers => Agent::ALL[0].into(),
        };
        self.start_making_for(agent, window, cx);
    }

    /// New Agent…, with `agent` chosen, over the Sandboxes place.
    pub(super) fn start_making_for(
        &mut self,
        agent: Harness,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page = Page::Sandboxes;
        if self.making.is_some() {
            self.choose_agent(agent, window, cx);
            return;
        }
        self.adding = None;
        self.imaging = None;
        self.microvm = None;
        let suggested = suggested_name(&agent, &[]);
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
        let harnesses = self.harnesses();
        let chosen = harnesses.iter().position(|known| *known == agent);
        let agents = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(harnesses.into_iter().map(AgentChoice).collect::<Vec<_>>()),
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
                this.choose_agent(agent.clone(), window, cx);
            },
        );
        let model = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Its own choice")));
        name.read(cx).focus_handle(cx).focus(window, cx);
        // On the gateway shown, until another is chosen.
        let gateway = self.gateway_key();
        let listing = self.list_for_making(&gateway, window, cx);
        self.making = Some(Making {
            gateway,
            agent,
            agents,
            name,
            model,
            suggested,
            folders: Vec::new(),
            listed: None,
            provider: None,
            profiles: None,
            services: BTreeSet::new(),
            kits: Vec::new(),
            variables: Vec::new(),
            setups: Vec::new(),
            overridden: Vec::new(),
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

    /// Asks the gateway `gateway` names for its providers and profiles, off the window's thread, for New Agent.
    fn list_for_making(
        &mut self,
        gateway: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let openshell = self.openshell_for(gateway);
        let watching = self.watch;
        cx.spawn_in(window, async move |this, cx| {
            if watching == Watch::Nothing {
                return;
            }
            let (listed, profiles) = cx
                .background_spawn(async move {
                    match openshell {
                        Ok(openshell) => (openshell.providers(), openshell.profiles().ok()),
                        Err(error) => (Err(error), None),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(making) = &mut this.making {
                    making.profiles = profiles;
                }
                this.providers_listed(listed.map_err(|error| error.to_string()), cx);
            });
        })
    }

    /// Makes the sandbox on the gateway `gateway` names instead, offering its providers.
    pub(super) fn choose_making_gateway(
        &mut self,
        gateway: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(making) = &self.making else {
            return;
        };
        if making.gateway == gateway {
            return;
        }
        let listing = self.list_for_making(&gateway, window, cx);
        if let Some(making) = &mut self.making {
            making.gateway = gateway;
            making.listed = None;
            making.profiles = None;
            making.provider = None;
            making._listing = listing;
        }
        cx.notify();
    }

    /// The gateways to make a sandbox on, when there's more than one, the chosen one marked.
    fn gateway_choices(&self, making: &Making, working: bool, cx: &Context<Self>) -> Div {
        h_flex()
            .flex_wrap()
            .gap_2()
            .children(self.gateways.iter().map(|gateway| {
                let name = gateway.name.clone();
                choice_chip(making.gateway == gateway.name, cx)
                    .id(ElementId::from((
                        ElementId::from("making-gateway"),
                        gateway.name.clone(),
                    )))
                    .child(gateway.name.clone())
                    .when(!working, |chip| {
                        chip.on_click(cx.listener(move |this, _, window, cx| {
                            this.choose_making_gateway(name.clone(), window, cx);
                        }))
                    })
            }))
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
        self.apply_kits(cx);
        cx.notify();
    }

    /// Chooses the agent, which may change the name Playpen suggests and the providers it can use.
    pub(super) fn choose_agent(
        &mut self,
        agent: Harness,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(making) = &mut self.making else {
            return;
        };
        if making.agent == agent {
            return;
        }
        making.agent = agent.clone();
        making.suggest_name(window, cx);
        // The menu shows the agent however it was chosen, from the menu or not.
        making.agents.update(cx, |agents, cx| {
            agents.set_selected_value(&agent, window, cx);
        });
        making.choose_first_provider();
        cx.notify();
    }

    /// Chooses the kit `name` for the new sandbox, or leaves it out if it's chosen already.
    pub(super) fn use_kit(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(making) = &mut self.making else {
            return;
        };
        if let Some(at) = making.kits.iter().position(|chosen| chosen == name) {
            making.kits.remove(at);
        } else if self.kits.kits.find(name).is_some() {
            making.kits.push(name.to_string());
        }
        self.apply_kits(cx);
        cx.notify();
    }

    /// Fills New Agent from the chosen kits: all their services, their variables, and the first model key among
    /// them that the agent can use.
    fn apply_kits(&mut self, _cx: &mut Context<Self>) {
        let Some(making) = &mut self.making else {
            return;
        };
        let kits: Vec<Kit> = making
            .kits
            .iter()
            .filter_map(|name| self.kits.kits.find(name).cloned())
            .collect();
        if kits.is_empty() {
            making.setups.clear();
            making.variables.clear();
            making.overridden.clear();
            return;
        }
        making.services = kits
            .iter()
            .flat_map(|kit| kit.services.iter().cloned())
            .collect();
        let mut variables: Vec<Variable> = Vec::new();
        let mut overridden = Vec::new();
        for variable in kits.iter().flat_map(|kit| kit.variables.iter()) {
            if let Some(at) = variables.iter().position(|set| set.name == variable.name) {
                overridden.push(variable.name.clone());
                variables[at] = variable.clone();
            } else {
                variables.push(variable.clone());
            }
        }
        making.variables = variables;
        making.overridden = overridden;
        making.setups = kits
            .iter()
            .filter_map(|kit| {
                Some(SetupFolder {
                    kit: kit.name.clone(),
                    folder: kit.setup_folder.clone()?.into(),
                })
            })
            .collect();
        if let Some(Ok(usable)) = making.usable() {
            let usable: Vec<String> = usable
                .iter()
                .map(|provider| provider.name.clone())
                .collect();
            if let Some(model_key) = kits
                .iter()
                .filter_map(|kit| kit.model_key.clone())
                .find(|model_key| usable.contains(model_key))
            {
                making.provider = Some(model_key);
            }
        }
    }

    /// The variables the chosen kits set, each with where its value comes from.
    fn kit_variables(&self, making: &Making, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        v_flex()
            .gap_1()
            .children(making.variables.iter().map(|variable| {
                let from = match &variable.value {
                    VariableValue::Value(value) => format!("= {value}"),
                    VariableValue::Reference(reference) => {
                        t!("from 1Password, %{reference}", reference = reference).into_owned()
                    }
                    VariableValue::Keychain(service) => {
                        t!("from the Keychain item “%{service}”", service = service).into_owned()
                    }
                };
                h_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_none()
                            .child(shell::code_chip(variable.name.clone(), cx)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(from),
                    )
                    .when_some(variable.check().err(), |row, problem| {
                        row.child(div().text_xs().text_color(theme.danger).child(problem))
                    })
            }))
            .when(!making.overridden.is_empty(), |list| {
                list.child(hint(
                    t!(
                        "Set by more than one kit, so the later one's is used: %{names}",
                        names = making.overridden.join(", ")
                    ),
                    cx,
                ))
            })
    }

    /// The kits as toggle chips with a leading check/plus icon, which fill the form when chosen.
    fn kit_choices(&self, making: &Making, working: bool, cx: &Context<Self>) -> Div {
        h_flex()
            .flex_wrap()
            .gap_2()
            .children(self.kits.kits.kits.iter().map(|kit| {
                let name = kit.name.clone();
                let picked = making.kits.contains(&kit.name);
                choice_chip(picked, cx)
                    .id(ElementId::from((
                        ElementId::from("use-kit"),
                        kit.name.clone(),
                    )))
                    .items_center()
                    .child(
                        Icon::new(if picked {
                            assets::IconName::Check
                        } else {
                            assets::IconName::Plus
                        })
                        .size_3(),
                    )
                    .child(kit.name.clone())
                    .when(!working, |chip| {
                        chip.on_click(cx.listener(move |this, _, _, cx| this.use_kit(&name, cx)))
                    })
            }))
    }

    /// Chooses a service, or leaves it out.
    fn toggle_service(&mut self, provider: String, chosen: bool, cx: &mut Context<Self>) {
        if let Some(making) = &mut self.making {
            if chosen {
                making.services.insert(provider);
            } else {
                making.services.remove(&provider);
            }
            cx.notify();
        }
    }

    /// The providers that can be services, each to tick, with the hosts it opens and how.
    fn service_choices(&self, making: &Making, working: bool, cx: &Context<Self>) -> Div {
        let (Some(Ok(providers)), Some(profiles)) = (&making.listed, &making.profiles) else {
            return hint(t!("Asking OpenShell…"), cx);
        };
        let choices = Service::choices(providers, profiles);
        if choices.is_empty() {
            return hint(
                t!(
                    "None on this gateway yet. Add one in Providers to let sandboxes reach services such as GitHub."
                ),
                cx,
            );
        }
        v_flex()
            .gap_2()
            .children(choices.into_iter().map(|(provider, profile)| {
                let name = provider.name.clone();
                let hosts: Vec<String> = profile
                    .endpoints
                    .iter()
                    .map(|endpoint| format!("{} ({})", endpoint.host, endpoint.access))
                    .collect();
                Checkbox::new(ElementId::from((ElementId::from("service"), name.clone())))
                    .label(format!("{name} · {} · {}", profile.name(), hosts.join(", ")))
                    .checked(making.services.contains(&name))
                    .disabled(working)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        this.toggle_service(name.clone(), *checked, cx);
                    }))
            }))
            .child(hint(
                t!(
                    "Any program in the sandbox can use a service's key, but only at its hosts, and only as its profile allows."
                ),
                cx,
            ))
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
        let Some(chosen) = self.making.as_ref().map(|making| making.gateway.clone()) else {
            return;
        };
        let openshell = self.openshell_for(&chosen);
        let gateway = self.gateway_info(&chosen);
        let settings = crate::settings::current(cx);
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
            agent: making.agent.clone(),
            // One of Playpen's agents starts from the person's own image when they gave one.
            image: match &making.agent {
                Harness::Agent(agent) => settings.agent_image(*agent),
                Harness::Custom(_) => None,
            },
            provider: making.provider.clone(),
            model: making.model(cx),
            account,
            services: making.chosen_services(),
            setups: making.setups.clone(),
            // Those that can't be set are shown with why, and left out.
            variables: making
                .variables
                .iter()
                .filter(|variable| variable.check().is_ok())
                .cloned()
                .collect(),
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
            let images =
                settings.image_source_for(&gateway, plan.names_its_image(), Docker::find)?;
            plan.make(&openshell, images.as_ref(), |step| {
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
                    this.guide_done(super::guide::GuideStep::MakeSandbox, window, cx);
                    this.selected = Some(SandboxKey {
                        gateway: chosen.clone(),
                        name,
                    });
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
                        .outline()
                        .xsmall()
                        .disabled(working)
                        .icon(assets::IconName::FolderPlus)
                        .label(t!("Add Folders…").into_owned())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.choose_folders(window, cx)),
                        ),
                ),
            );
        let agent = making.agent.name().to_string();
        let providers = match making.usable() {
            None => hint(t!("Asking OpenShell…"), cx),
            Some(Err(error)) => div()
                .text_sm()
                .text_color(theme.danger)
                .child(error.clone()),
            Some(Ok(providers)) if providers.is_empty() => v_flex()
                .gap_2()
                .items_start()
                .child(hint(
                    t!(
                        "The %{gateway} gateway has no provider with a key for %{agent}. Choose another gateway, or add one in Providers.",
                        gateway = making.gateway.clone(),
                        agent = agent.clone()
                    ),
                    cx,
                ))
                .child(
                    Button::new("making-add-provider")
                        .outline()
                        .xsmall()
                        .icon(assets::IconName::Plus)
                        .label(t!("Open Providers…").into_owned())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.show_page(Page::Providers, window, cx);
                        })),
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
        let footer_left = match &making.state {
            MakeState::Choosing => div().into_any_element(),
            MakeState::Working(step) => h_flex()
                .gap_2()
                .items_center()
                .child(Spinner::new().xsmall())
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(
                            match step {
                                Step::Building => t!(
                                    "Building the %{agent} image (first time takes a few minutes)…",
                                    agent = making.agent.name()
                                ),
                                Step::Pushing => t!("Pushing the image to the registry…"),
                                Step::Creating => t!("Making the sandbox…"),
                                Step::Copying => t!("Copying the folders in…"),
                                Step::Preparing => {
                                    t!("Setting up %{agent}…", agent = making.agent.name())
                                }
                            }
                            .into_owned(),
                        ),
                )
                .into_any_element(),
            MakeState::Failed(error) => div()
                .text_sm()
                .text_color(theme.danger)
                .child(error.clone())
                .into_any_element(),
        };
        let body = v_flex()
            .gap_5()
            .p_4()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_4()
                    .items_start()
                    .child(
                        section(t!("Agent"), cx).child(
                            div().w(rems(15.)).child(
                                Select::new(&making.agents)
                                    .small()
                                    .search_placeholder(t!("Search").into_owned())
                                    .disabled(working),
                            ),
                        ),
                    )
                    .child(
                        section(t!("Name"), cx)
                            .child(div().w(rems(16.)).child(small_field(&making.name))),
                    ),
            )
            .child(section(t!("Folders"), cx).child(folders))
            .when(self.gateways.len() > 1, |body| {
                body.child(
                    section(t!("Gateway"), cx).child(self.gateway_choices(making, working, cx)),
                )
            })
            .when(!self.kits.kits.kits.is_empty(), |body| {
                body.child(section(t!("Kits"), cx).child(self.kit_choices(making, working, cx)))
            })
            .when(!making.variables.is_empty(), |body| {
                body.child(section(t!("Variables"), cx).child(self.kit_variables(making, cx)))
            })
            .child(section(t!("Model key"), cx).child(providers))
            .when(making.agent.chooses_a_model(), |body| {
                body.child(
                    section(t!("Model"), cx)
                        .child(div().w(rems(16.)).child(small_field(&making.model)))
                        .child(hint(
                            t!(
                                "As the service names it, such as anthropic/claude-sonnet-4.5 on OpenRouter. A LiteLLM proxy needs one; any other key starts with its service's own."
                            ),
                            cx,
                        )),
                )
            })
            .child(section(t!("Services"), cx).child(self.service_choices(making, working, cx)))
            .child(hint(
                t!(
                    "Folders are copied into /sandbox (respecting .gitignore) with git history intact so Bring Back returns your agent's work as playpen/<sandbox>. Keys stay in OpenShell."
                ),
                cx,
            ));
        let cancel = Button::new("cancel-making")
            .ghost()
            .small()
            .disabled(working)
            .label(t!("Cancel").into_owned())
            .on_click(cx.listener(|this, _, window, cx| this.cancel_making(window, cx)));
        let primary = Button::new("make")
            .primary()
            .small()
            .loading(working)
            .disabled(working || making.provider.is_none())
            .label(t!("Make Sandbox").into_owned())
            .on_click(cx.listener(|this, _, window, cx| this.make(window, cx)));
        self.form_panel(
            t!("New Agent"),
            "making",
            body,
            footer_left,
            cancel,
            primary,
            working,
            |this, _, window, cx| this.cancel_making(window, cx),
            cx,
        )
    }
}

impl Making {
    /// The services chosen, with the hosts their profiles open.
    fn chosen_services(&self) -> Vec<Service> {
        let (Some(Ok(providers)), Some(profiles)) = (&self.listed, &self.profiles) else {
            return Vec::new();
        };
        Service::choices(providers, profiles)
            .into_iter()
            .filter(|(provider, _)| self.services.contains(&provider.name))
            .map(|(provider, profile)| Service::new(provider, profile))
            .collect()
    }

    /// Names the sandbox after the agent and the folders, unless the person has typed a name of their own.
    fn suggest_name(&mut self, window: &mut Window, cx: &mut App) {
        let typed = self.name.read(cx).value().trim().to_string();
        if typed == self.suggested || typed.is_empty() {
            let suggested = suggested_name(&self.agent, &self.folders);
            self.name
                .update(cx, |name, cx| name.set_value(suggested.clone(), window, cx));
            self.suggested = suggested;
        }
    }

    /// The providers whose key the agent can use, once OpenShell has listed them.
    fn usable(&self) -> Option<Result<Vec<&Provider>, &String>> {
        self.listed.as_ref().map(|listed| {
            listed.as_ref().map(|providers| {
                providers
                    .iter()
                    .filter(|provider| self.agent.takes(&provider.kind))
                    .collect()
            })
        })
    }

    /// The chosen key as Pi and its kin need it, with the model typed, for an agent that chooses one.
    fn model(&self, cx: &App) -> Option<Model> {
        if !self.agent.chooses_a_model() {
            return None;
        }
        let name = self.provider.as_ref()?;
        let provider = self
            .usable()?
            .ok()?
            .into_iter()
            .find(|provider| &provider.name == name)?;
        let typed = self.model.read(cx).value().to_string();
        Model::of(
            provider,
            self.profiles.as_deref().unwrap_or_default(),
            Some(typed),
        )
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
            .context("Sign the ChatGPT provider in again in Providers")
    }
}

#[cfg(test)]
impl Making {
    #[cfg(test)]
    pub(super) fn variable_values(&self) -> Vec<String> {
        self.variables
            .iter()
            .map(|variable| match &variable.value {
                VariableValue::Value(value) => format!("{}={value}", variable.name),
                VariableValue::Reference(reference) => format!("{}={reference}", variable.name),
                VariableValue::Keychain(service) => format!("{}=keychain:{service}", variable.name),
            })
            .collect()
    }

    #[cfg(test)]
    pub(super) fn ticked_services(&self) -> Vec<&str> {
        self.services.iter().map(String::as_str).collect()
    }

    pub(super) fn chosen_provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    pub(super) fn name_typed(&self, cx: &App) -> String {
        self.name.read(cx).value().to_string()
    }

    pub(super) fn model_for_plan(&self, cx: &App) -> Option<Model> {
        self.model(cx)
    }
}

#[cfg(test)]
impl MainView {
    pub(super) fn type_model(&mut self, model: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(making) = &self.making {
            making.model.update(cx, |field, cx| {
                field.set_value(model.to_string(), window, cx)
            });
        }
    }
}
