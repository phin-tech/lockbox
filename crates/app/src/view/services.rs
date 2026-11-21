//! The chosen sandbox's services: providers other than its model's key, attached while it runs and detached again.
//! What's attached is asked of OpenShell for the chosen sandbox only, when it's chosen and after each change, rather
//! than every few seconds for them all.

use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use playpen_core::{Profile, Provider, Service};

use super::sandboxes::text_value;
use super::*;

/// What OpenShell said about the chosen sandbox's services.
#[derive(Default)]
pub(super) struct Services {
    /// The sandbox it's about.
    sandbox: Option<String>,
    /// The providers attached to it, by name, or why they couldn't be listed.
    attached: Option<Result<Vec<String>, String>>,
    /// Every provider and profile, to say which can be attached.
    catalog: Option<(Vec<Provider>, Vec<Profile>)>,
    /// Attaching or detaching one, while it's done.
    changing: Option<String>,
    _asking: Option<Task<()>>,
    _changing: Option<Task<()>>,
}

impl MainView {
    /// Asks OpenShell what's attached to the chosen sandbox, and what could be. A test or a capture doesn't ask.
    pub(super) fn look_up_services(&mut self, cx: &mut Context<Self>) {
        if self.watch == Watch::Nothing {
            return;
        }
        let Some(name) = self.selected.clone() else {
            return;
        };
        if self.services.sandbox.as_ref() != Some(&name) {
            self.services.attached = None;
        }
        self.services.sandbox = Some(name.clone());
        let Ok(openshell) = self.openshell() else {
            return;
        };
        self.services._asking = Some(cx.spawn(async move |this, cx| {
            let (attached, catalog) = cx
                .background_spawn({
                    let name = name.clone();
                    async move {
                        let attached = openshell
                            .attached(&name)
                            .map(|providers| {
                                providers
                                    .into_iter()
                                    .map(|provider| provider.name)
                                    .collect()
                            })
                            .map_err(|error| error.to_string());
                        let catalog = openshell.providers().ok().zip(openshell.profiles().ok());
                        (attached, catalog)
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Only if it's still the one chosen.
                if this.services.sandbox.as_ref() == Some(&name) {
                    this.services.attached = Some(attached);
                    if catalog.is_some() {
                        this.services.catalog = catalog;
                    }
                    cx.notify();
                }
            });
        }));
    }

    /// Attaches `service` to the chosen sandbox, or detaches it, off the window's thread.
    fn change_service(&mut self, service: Service, attach: bool, cx: &mut Context<Self>) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        if self.services.changing.is_some() {
            return;
        }
        let Ok(openshell) = self.openshell() else {
            return;
        };
        self.failed = None;
        self.services.changing = Some(service.provider.clone());
        tracing::info!(sandbox = %name, service = %service.provider, attach, "changing a service");
        self.services._changing = Some(cx.spawn(async move |this, cx| {
            let done = cx
                .background_spawn({
                    let name = name.clone();
                    async move {
                        if attach {
                            openshell.attach_service(&name, &service)
                        } else {
                            openshell.detach_service(&name, &service)
                        }
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.services.changing = None;
                if let Err(error) = done {
                    tracing::warn!(sandbox = %name, "couldn't change a service: {error}");
                    this.failed = Some(error.to_string());
                }
                this.look_up_services(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The services that can be attached, with their profiles.
    fn service_catalog(&self) -> Vec<Service> {
        let Some((providers, profiles)) = &self.services.catalog else {
            return Vec::new();
        };
        Service::choices(providers, profiles)
            .into_iter()
            .map(|(provider, profile)| Service::new(provider, profile))
            .collect()
    }

    /// The chosen sandbox's Services row: each attached, with a button to detach it, and Attach… for the rest.
    pub(super) fn services_row(&self, sandbox: &Sandbox, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let ready = sandbox.phase.is_ready();
        let changing = self.services.changing.clone();
        let catalog = self.service_catalog();
        let attached: Vec<String> = match &self.services.attached {
            Some(Ok(attached)) => attached
                .iter()
                .filter(|name| catalog.iter().any(|service| &service.provider == *name))
                .cloned()
                .collect(),
            Some(Err(_)) => return text_value(t!("OpenShell isn't answering").into_owned()),
            None if self.watch == Watch::Nothing => {
                return text_value(t!("Not asked").into_owned());
            }
            None => return text_value(t!("Asking OpenShell…").into_owned()),
        };
        let spare: Vec<Service> = catalog
            .iter()
            .filter(|service| !attached.contains(&service.provider))
            .cloned()
            .collect();
        let view = cx.entity().downgrade();
        let attach = Button::new("attach-service")
            .xsmall()
            .icon(assets::IconName::Plus)
            .label(t!("Attach…").into_owned())
            .disabled(!ready || changing.is_some() || spare.is_empty())
            .loading(
                changing
                    .as_ref()
                    .is_some_and(|name| !attached.contains(name)),
            )
            .dropdown_menu(move |mut menu, _, _| {
                for service in &spare {
                    let hosts: Vec<&str> = service
                        .endpoints
                        .iter()
                        .map(|endpoint| endpoint.host.as_str())
                        .collect();
                    let view = view.clone();
                    let service = service.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("{} · {}", service.provider, hosts.join(", ")))
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    this.change_service(service.clone(), true, cx);
                                });
                            }),
                    );
                }
                menu
            });
        v_flex()
            .min_w_0()
            .gap_1()
            .items_end()
            .when(attached.is_empty(), |list| {
                list.child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(t!("None").into_owned()),
                )
            })
            .children(attached.into_iter().map(|name| {
                let service = catalog
                    .iter()
                    .find(|service| service.provider == name)
                    .cloned();
                h_flex()
                    .gap_1()
                    .child(shell::code_chip(name.clone(), cx))
                    .when_some(service, |row, service| {
                        row.child(
                            Button::new(ElementId::from((ElementId::from("detach"), name.clone())))
                                .ghost()
                                .xsmall()
                                .icon(assets::IconName::X)
                                .tooltip(t!("Detach").into_owned())
                                .loading(changing.as_ref() == Some(&name))
                                .disabled(!ready || changing.is_some())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.change_service(service.clone(), false, cx);
                                })),
                        )
                    })
            }))
            .child(h_flex().child(attach))
    }
}
