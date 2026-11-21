//! New Agent…: a sandbox made for an agent, in place of the chosen sandbox. The person names it, chooses a folder to
//! copy in and the provider holding the model's key; making it runs off the window's thread, a step at a time.

use std::path::PathBuf;

use futures::StreamExt as _;
use futures::channel::mpsc;
use lockbox_core::{Docker, Plan, Provider, Step, suggested_name};

use super::*;

/// A sandbox being made for an agent.
pub struct Making {
    agent: Agent,
    name: Entity<InputState>,
    /// The name Lockbox last suggested, which choosing a folder may change while the person hasn't typed another.
    suggested: String,
    folder: Option<PathBuf>,
    /// The providers that hold a key the agent can use, once OpenShell has listed them.
    providers: Option<Result<Vec<Provider>, String>>,
    provider: Option<String>,
    pub(super) state: MakeState,
    _changed: Subscription,
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
        let suggested = suggested_name(agent, None);
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
            name,
            suggested,
            folder: None,
            providers: None,
            provider: None,
            state: MakeState::Choosing,
            _changed: changed,
            _listing: listing,
            _making: None,
        });
        self.failed = None;
        cx.notify();
    }

    /// Takes in the providers OpenShell listed, keeping those whose key the agent can use, and chooses the first.
    pub(super) fn providers_listed(
        &mut self,
        listed: Result<Vec<Provider>, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(making) = &mut self.making else {
            return;
        };
        let kinds = making.agent.provider_kinds();
        let usable = listed.map(|providers| {
            providers
                .into_iter()
                .filter(|provider| kinds.contains(&provider.kind.as_str()))
                .collect::<Vec<_>>()
        });
        making.provider = usable
            .as_ref()
            .ok()
            .and_then(|providers| providers.first())
            .map(|provider| provider.name.clone());
        making.providers = Some(usable);
        cx.notify();
    }

    /// Closes New Agent…, giving the keys back to the list, since its name field had them.
    pub(super) fn cancel_making(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.making = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Asks for the folder to copy in, and names the sandbox after it unless the person has named it.
    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("Choose").into_owned().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.folder_chosen(folder, window, cx)
            });
        })
        .detach();
    }

    pub(super) fn folder_chosen(
        &mut self,
        folder: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(making) = &mut self.making else {
            return;
        };
        let typed = making.name.read(cx).value().trim().to_string();
        if typed == making.suggested || typed.is_empty() {
            let suggested = suggested_name(making.agent, Some(&folder));
            making
                .name
                .update(cx, |name, cx| name.set_value(suggested.clone(), window, cx));
            making.suggested = suggested;
        }
        making.folder = Some(folder);
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
        let plan = Plan {
            name: making.name.read(cx).value().trim().to_string(),
            agent: making.agent,
            provider: making.provider.clone(),
            folder: making.folder.clone(),
        };
        if !lockbox_core::is_sandbox_name(&plan.name) {
            making.state = MakeState::Failed(lockbox_core::MakeError::Name(plan.name).to_string());
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
        let folder = h_flex()
            .gap_2()
            .child(match &making.folder {
                Some(folder) => h_flex()
                    .min_w_0()
                    .child(shell::code_chip(folder.display().to_string(), cx)),
                None => hint(t!("None: it starts empty"), cx),
            })
            .child(
                Button::new("choose-folder")
                    .xsmall()
                    .disabled(working)
                    .label(t!("Choose…").into_owned())
                    .on_click(cx.listener(|this, _, window, cx| this.choose_folder(window, cx))),
            );
        let providers = match &making.providers {
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
                            .lockbox_kind()
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
                    "The folder is copied in, leaving out what its .gitignore does, so the agent works on the copy. The provider's key never enters the sandbox: OpenShell puts it on the agent's requests to its model, and the agent can reach nothing else."
                ),
                cx,
            ),
            MakeState::Working(step) => h_flex().gap_2().child(div().child(match step {
                Step::Building => t!(
                    "Building the %{agent} image. This happens once for each version, and takes a few minutes…",
                    agent = making.agent.name()
                ),
                Step::Creating => t!("Making the sandbox…"),
                Step::Copying => t!("Copying the folder in…"),
            }
            .into_owned())),
            MakeState::Failed(error) => div().text_sm().text_color(theme.danger).child(error.clone()),
        };
        let body = v_flex()
            .gap_5()
            .p_4()
            .child(section(t!("Agent"), cx).child(div().child(making.agent.name())))
            .child(
                section(t!("Name"), cx).child(div().w(rems(20.)).child(small_field(&making.name))),
            )
            .child(section(t!("Folder"), cx).child(folder))
            .child(section(t!("Model key"), cx).child(providers))
            .child(state);
        v_flex()
            .size_full()
            .child(header)
            .child(div().id("making").flex_1().overflow_y_scroll().child(body))
            .into_any_element()
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
