//! Gateway ▸ Set Up MicroVM Gateway…: a gateway on this Mac whose sandboxes are small virtual machines, which Playpen
//! sets up and runs, in place of the chosen sandbox. It says what this Mac is missing and installs e2fsprogs when
//! asked, lets the size of each microVM be chosen, and shows the gateway once it's running. Removing it is Remove
//! Gateway…, which stops it as well.

use playpen_core::{
    MICROVM_GATEWAY, MicroVm, MicroVmError, MicroVmSize, Missing, SetUpStep, install_e2fsprogs,
    why_not_started,
};

use super::*;

/// The MicroVM gateway being set up.
pub struct MicroVmForm {
    /// The gateway Playpen would run, or none when OpenShell isn't installed.
    microvm: Option<MicroVm>,
    missing: Vec<Missing>,
    size: MicroVmSize,
    state: SetUpState,
    _working: Option<Task<()>>,
}

enum SetUpState {
    Choosing,
    Installing,
    Working(SetUpStep),
    Failed(String),
}

impl MicroVmForm {
    fn busy(&self) -> bool {
        matches!(self.state, SetUpState::Installing | SetUpState::Working(_))
    }
}

/// Which of the sizes a chip chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SizePart {
    Cpus,
    Memory,
    Disk,
}

impl MainView {
    /// Shows the form, with what this Mac is missing.
    pub(super) fn start_microvm_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.microvm.as_ref().is_some_and(MicroVmForm::busy) {
            return;
        }
        // A test or a capture doesn't look at this Mac.
        let microvm = (self.watch != Watch::Nothing)
            .then(|| self.openshell().ok())
            .flatten()
            .and_then(|openshell| MicroVm::find(&openshell));
        let missing = microvm
            .as_ref()
            .map_or_else(|| vec![Missing::Driver], MicroVm::missing);
        // Setting it up again starts from the size it has.
        let size = microvm
            .as_ref()
            .and_then(MicroVm::configured_size)
            .unwrap_or_default();
        self.making = None;
        self.adding = None;
        self.imaging = None;
        self.failed = None;
        self.microvm = Some(MicroVmForm {
            microvm,
            missing,
            size,
            state: SetUpState::Choosing,
            _working: None,
        });
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn cancel_microvm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.microvm.as_ref().is_some_and(MicroVmForm::busy) {
            return;
        }
        self.microvm = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn choose_microvm_size(
        &mut self,
        part: SizePart,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &mut self.microvm else {
            return;
        };
        match part {
            SizePart::Cpus => form.size.cpus = value,
            SizePart::Memory => form.size.memory_gib = value,
            SizePart::Disk => form.size.disk_gib = value,
        }
        cx.notify();
    }

    /// Installs e2fsprogs with Homebrew, off the window's thread, then looks again at what's missing.
    fn install_e2fsprogs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.microvm else {
            return;
        };
        if form.busy() {
            return;
        }
        tracing::info!("installing e2fsprogs");
        form.state = SetUpState::Installing;
        let done = cx.background_spawn(async move { install_e2fsprogs() });
        form._working = Some(cx.spawn_in(window, async move |this, cx| {
            let done = done.await;
            let _ = this.update(cx, |this, cx| {
                let Some(form) = &mut this.microvm else {
                    return;
                };
                form.missing = form
                    .microvm
                    .as_ref()
                    .map_or_else(|| vec![Missing::Driver], MicroVm::missing);
                form.state = match done {
                    Ok(()) => SetUpState::Choosing,
                    Err(error) => {
                        tracing::warn!("couldn't install e2fsprogs: {error}");
                        SetUpState::Failed(error.to_string())
                    }
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Sets the gateway up off the window's thread, saying each step, and shows it once OpenShell knows it.
    pub(super) fn set_up_microvm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let openshell = self.openshell();
        let Some(form) = &mut self.microvm else {
            return;
        };
        if form.busy() || !form.missing.is_empty() {
            return;
        }
        let Some(microvm) = form.microvm.clone() else {
            return;
        };
        let size = form.size;
        tracing::info!(?size, "setting up the MicroVM gateway");
        form.state = SetUpState::Working(SetUpStep::Certificates);
        let (steps, mut heard) = futures::channel::mpsc::unbounded();
        let done = cx.background_spawn(async move {
            let places = microvm.places().clone();
            microvm
                .set_up(size, |step| {
                    let _ = steps.unbounded_send(step);
                })
                .map_err(|error| match error {
                    // What the gateway said is more use than where it said it.
                    MicroVmError::NotStarted(..) => {
                        let tail = why_not_started(&places);
                        if tail.is_empty() {
                            error.to_string()
                        } else {
                            format!("{error}\n{tail}")
                        }
                    }
                    error => error.to_string(),
                })?;
            openshell
                .and_then(|openshell| openshell.gateways())
                .map_err(|error| error.to_string())
        });
        form._working = Some(cx.spawn_in(window, async move |this, cx| {
            let listening = {
                let this = this.clone();
                let mut cx = cx.clone();
                async move {
                    while let Some(step) = futures::StreamExt::next(&mut heard).await {
                        let _ = this.update(&mut cx, |this, cx| {
                            if let Some(form) = &mut this.microvm {
                                form.state = SetUpState::Working(step);
                                cx.notify();
                            }
                        });
                    }
                }
            };
            let (done, ()) = futures::join!(done, listening);
            let _ = this.update_in(cx, |this, window, cx| {
                match done {
                    Ok(gateways) => {
                        tracing::info!("set up the MicroVM gateway");
                        this.microvm = None;
                        this.gateways_listed(Ok(gateways));
                        this.show_gateway(MICROVM_GATEWAY, window, cx);
                        this.focus.focus(window, cx);
                    }
                    Err(error) => {
                        tracing::warn!("couldn't set up the MicroVM gateway: {error}");
                        if let Some(form) = &mut this.microvm {
                            form.state = SetUpState::Failed(error);
                        }
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Asks before removing the MicroVM gateway Playpen set up, which stops it and its sandboxes.
    pub(super) fn confirm_remove_microvm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(microvm) = self
            .openshell()
            .ok()
            .and_then(|openshell| MicroVm::find(&openshell))
        else {
            return;
        };
        let folder = microvm.places().root.display().to_string();
        let remove = t!("Remove");
        let answer = window.prompt(
            PromptLevel::Warning,
            &t!("Remove the MicroVM gateway?"),
            Some(&t!(
                "It stops, and so do its sandboxes. Its certificates and its sandboxes' disks stay in %{folder}.",
                folder = folder
            )),
            &[&*remove, &*t!("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.remove_microvm(microvm, window, cx);
                });
            }
        })
        .detach();
    }

    fn remove_microvm(&mut self, microvm: MicroVm, window: &mut Window, cx: &mut Context<Self>) {
        let openshell = self.openshell();
        self.failed = None;
        tracing::info!("removing the MicroVM gateway");
        let done = cx.background_spawn(async move {
            let kept = microvm.remove().map_err(|error| error.to_string())?;
            let gateways = openshell
                .and_then(|openshell| openshell.gateways())
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((kept, gateways))
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let done = done.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.gateway_working = None;
                match done {
                    Ok((kept, gateways)) => {
                        tracing::info!("removed the MicroVM gateway");
                        crate::settings::set_gateway(MICROVM_GATEWAY, None, cx);
                        let was_shown = this.gateway.name() == Some(MICROVM_GATEWAY);
                        this.gateways_listed(Ok(gateways));
                        Toast::new(
                            t!(
                                "Removed MicroVM gateway (disks kept in %{folder})",
                                folder = kept.display().to_string()
                            )
                            .into_owned(),
                        )
                        .key("microvm-removed")
                        .show(window, cx);
                        if was_shown {
                            cx.set_global(crate::providers::ShownGateway(this.gateway.clone()));
                        }
                        this.refresh(window, cx);
                    }
                    Err(error) => {
                        tracing::warn!("couldn't remove the MicroVM gateway: {error}");
                        this.failed = Some(error);
                    }
                }
                cx.notify();
            });
        });
        self.gateway_working = Some(gateways::GatewayWork::new(
            t!("Stopping the MicroVM gateway…").into_owned(),
            task,
        ));
        cx.notify();
    }

    /// A row of chips choosing one of `values` for `part`, each said by `label`.
    fn size_chips(
        &self,
        form: &MicroVmForm,
        part: SizePart,
        values: &[u32],
        label: impl Fn(u32) -> String,
        cx: &Context<Self>,
    ) -> Div {
        let chosen = match part {
            SizePart::Cpus => form.size.cpus,
            SizePart::Memory => form.size.memory_gib,
            SizePart::Disk => form.size.disk_gib,
        };
        let busy = form.busy();
        h_flex().gap_2().children(values.iter().map(|value| {
            let value = *value;
            choice_chip(chosen == value, cx)
                .id(ElementId::from((
                    ElementId::from(format!("microvm-{part:?}")),
                    value.to_string(),
                )))
                .child(label(value))
                .when(!busy, |chip| {
                    chip.on_click(cx.listener(move |this, _, _, cx| {
                        this.choose_microvm_size(part, value, cx);
                    }))
                })
        }))
    }

    /// What this Mac has and hasn't of what the gateway needs, with a way to get each that's missing.
    fn requirements(&self, form: &MicroVmForm, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let row = |missing: Missing, name: String, how: String| {
            let has = !form.missing.contains(&missing);
            h_flex()
                .gap_2()
                .items_start()
                .child(
                    Icon::new(if has {
                        assets::IconName::Check
                    } else {
                        assets::IconName::X
                    })
                    .size_4()
                    .text_color(if has {
                        theme.success
                    } else {
                        theme.danger
                    }),
                )
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(div().text_sm().child(name))
                        .when(!has, |row| row.child(hint(how, cx))),
                )
        };
        let installing = matches!(form.state, SetUpState::Installing);
        v_flex()
            .gap_2()
            .child(row(
                Missing::Driver,
                t!("OpenShell's MicroVM driver").into_owned(),
                t!("It comes with OpenShell from Homebrew: brew install openshell").into_owned(),
            ))
            .child(
                row(
                    Missing::E2fsprogs,
                    t!("e2fsprogs").into_owned(),
                    t!("It makes the microVMs' disks.").into_owned(),
                )
                .when(form.missing.contains(&Missing::E2fsprogs), |row| {
                    row.child(
                        Button::new("install-e2fsprogs")
                            .xsmall()
                            .icon(assets::IconName::Download)
                            .label(t!("Install with Homebrew").into_owned())
                            .loading(installing)
                            .disabled(form.busy())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.install_e2fsprogs(window, cx);
                            })),
                    )
                }),
            )
            .child(row(
                Missing::Docker,
                t!("Docker").into_owned(),
                t!("Agents' images are built in Docker, and the microVMs start from them. OrbStack or Docker Desktop will do.").into_owned(),
            ))
    }

    pub(super) fn microvm_panel(&self, form: &MicroVmForm, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let busy = form.busy();
        let set_up_before = form.microvm.as_ref().is_some_and(MicroVm::is_set_up);
        let gib = |value: u32| t!("%{count} GB", count = value).into_owned();
        let footer_left = match &form.state {
            SetUpState::Choosing | SetUpState::Installing => div().into_any_element(),
            SetUpState::Working(step) => h_flex()
                .gap_2()
                .items_center()
                .child(Spinner::new().xsmall())
                .child(hint(
                    match step {
                        SetUpStep::Certificates => t!("Making its certificates…"),
                        SetUpStep::Starting => t!("Starting it…"),
                        SetUpStep::Registering => t!("Registering it with OpenShell…"),
                    },
                    cx,
                ))
                .into_any_element(),
            SetUpState::Failed(error) => div()
                .text_sm()
                .text_color(theme.danger)
                .child(error.clone())
                .into_any_element(),
        };
        let folder = form
            .microvm
            .as_ref()
            .map(|microvm| microvm.places().root.display().to_string())
            .unwrap_or_default();
        let body = v_flex()
            .gap_5()
            .p_4()
            .child(hint(
                t!(
                    "A gateway on this Mac whose sandboxes are small virtual machines, each with a Linux kernel of its own, rather than containers in Docker. It runs beside the gateway you have, keeps running when Playpen quits, and starts again with the Mac. OpenShell calls its MicroVM driver experimental."
                ),
                cx,
            ))
            .child(section(t!("Needs"), cx).child(self.requirements(form, cx)))
            .child(
                section(t!("Each sandbox gets"), cx)
                    .child(self.size_chips(
                        form,
                        SizePart::Cpus,
                        &MicroVmSize::CPUS,
                        |value| t!("%{count} CPUs", count = value).into_owned(),
                        cx,
                    ))
                    .child(self.size_chips(
                        form,
                        SizePart::Memory,
                        &MicroVmSize::MEMORY_GIB,
                        |value| t!("%{memory} memory", memory = gib(value)).into_owned(),
                        cx,
                    ))
                    .child(self.size_chips(
                        form,
                        SizePart::Disk,
                        &MicroVmSize::DISK_GIB,
                        |value| t!("%{disk} disk", disk = gib(value)).into_owned(),
                        cx,
                    ))
                    .child(hint(
                        t!("Memory is taken as a sandbox uses it, and disk as it's written."),
                        cx,
                    )),
            )
            .when(set_up_before, |body| {
                body.child(
                    div().text_sm().text_color(theme.warning).child(
                        t!("It's set up already. Setting it up again restarts it and its sandboxes, which come back with their disks; what they hadn't saved yet is lost.")
                            .into_owned(),
                    ),
                )
            })
            .child(hint(
                t!(
                    "Its certificates, and its sandboxes' disks, are kept in %{folder}. The first sandbox takes a minute or so while its image is prepared; after that, one starts in seconds.",
                    folder = folder
                ),
                cx,
            ));
        let cancel = Button::new("cancel-microvm")
            .ghost()
            .small()
            .disabled(busy)
            .label(t!("Cancel").into_owned())
            .on_click(cx.listener(|this, _, window, cx| this.cancel_microvm(window, cx)));
        let primary = Button::new("set-up-microvm")
            .primary()
            .small()
            .disabled(busy || !form.missing.is_empty())
            .loading(matches!(form.state, SetUpState::Working(_)))
            .label(if set_up_before {
                t!("Set Up Again").into_owned()
            } else {
                t!("Set Up Gateway").into_owned()
            })
            .on_click(cx.listener(|this, _, window, cx| this.set_up_microvm(window, cx)));
        self.form_panel(
            t!("MicroVM Gateway"),
            "microvm",
            body,
            footer_left,
            cancel,
            primary,
            busy,
            |this, _, window, cx| this.cancel_microvm(window, cx),
            cx,
        )
    }
}

#[cfg(test)]
impl MicroVmForm {
    pub(super) fn size(&self) -> MicroVmSize {
        self.size
    }

    pub(super) fn failure(&self) -> Option<&str> {
        match &self.state {
            SetUpState::Failed(error) => Some(error),
            _ => None,
        }
    }
}
