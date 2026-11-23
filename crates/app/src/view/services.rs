//! The chosen sandbox's services: providers other than its model's key, attached while it runs and detached again.
//! What's attached is asked of OpenShell for the chosen sandbox only, when it's chosen and after each change, rather
//! than every few seconds for them all.

use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use playpen_core::{Profile, Provider, Service};

use super::sandboxes::text_value;
use super::*;

/// What choosing an item from the sandbox's Attach autocomplete does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AttachTarget {
    Service(Service),
    Kit(String),
    NewService,
}

/// An option inside the searchable Attach autocomplete list.
#[derive(Clone)]
pub(super) struct AttachChoice {
    label: SharedString,
    target: AttachTarget,
}

impl SelectItem for AttachChoice {
    type Value = AttachTarget;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &AttachTarget {
        &self.target
    }
}

/// What OpenShell said about the chosen sandbox's services.
pub(super) struct Services {
    /// The sandbox it's about.
    sandbox: Option<SandboxKey>,
    /// The providers attached to it, by name, or why they couldn't be listed.
    attached: Option<Result<Vec<String>, String>>,
    /// Every provider and profile, to say which can be attached.
    pub(super) catalog: Option<(Vec<Provider>, Vec<Profile>)>,
    /// Attaching or detaching one, while it's done.
    changing: Option<String>,
    /// Searchable autocomplete picker for attaching services, kits, or adding a new service for this sandbox.
    picker: Entity<SelectState<SearchableVec<AttachChoice>>>,
    /// Set whenever the chosen sandbox, attached list, catalog, or kits change so the picker refreshes on render.
    pub(super) picker_dirty: bool,
    /// When `+ New Service…` was picked from a sandbox, attach the newly created service to just this sandbox.
    pub(super) pending_attach: Option<SandboxKey>,
    _picked: Subscription,
    _asking: Option<Task<()>>,
    _changing: Option<Task<()>>,
}

impl Services {
    pub(super) fn new(window: &mut Window, cx: &mut Context<MainView>) -> Self {
        let picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(Vec::<AttachChoice>::new()),
                None,
                window,
                cx,
            )
            .searchable(true)
        });
        let picked = cx.subscribe_in(
            &picker,
            window,
            |this, picker, event: &SelectEvent<SearchableVec<AttachChoice>>, window, cx| {
                let SelectEvent::Confirm(Some(target)) = event else {
                    return;
                };
                let target = target.clone();
                // Keep the combobox ready for the next attachment rather than showing the last picked choice.
                picker.update(cx, |state, cx| state.set_selected_index(None, window, cx));
                match target {
                    AttachTarget::Service(service) => {
                        this.change_service(service, true, window, cx);
                    }
                    AttachTarget::Kit(kit) => {
                        this.attach_kit(&kit, window, cx);
                    }
                    AttachTarget::NewService => {
                        this.start_new_service_for_selected(window, cx);
                    }
                }
            },
        );
        Self {
            sandbox: None,
            attached: None,
            catalog: None,
            changing: None,
            picker,
            picker_dirty: true,
            pending_attach: None,
            _picked: picked,
            _asking: None,
            _changing: None,
        }
    }
}

impl MainView {
    /// Asks OpenShell what's attached to the chosen sandbox, and what could be. A test or a capture doesn't ask.
    pub(super) fn look_up_services(&mut self, cx: &mut Context<Self>) {
        if self.watch == Watch::Nothing {
            return;
        }
        self.look_up_repos(cx);
        let Some(key) = self.selected.clone() else {
            return;
        };
        if self.services.sandbox.as_ref() != Some(&key) {
            self.services.attached = None;
            self.services.picker_dirty = true;
        }
        self.services.sandbox = Some(key.clone());
        let Ok(openshell) = self.openshell_for(&key.gateway) else {
            return;
        };
        self.services._asking = Some(cx.spawn(async move |this, cx| {
            let (attached, catalog) = cx
                .background_spawn({
                    let name = key.name.clone();
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
                if this.services.sandbox.as_ref() == Some(&key) {
                    this.services.attached = Some(attached);
                    if catalog.is_some() {
                        this.services.catalog = catalog;
                    }
                    this.services.picker_dirty = true;
                    cx.notify();
                }
            });
        }));
    }

    /// Rebuilds the autocomplete list of services, kits, and `+ New Service…` for the chosen sandbox.
    pub(super) fn sync_attach_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.services.picker_dirty {
            return;
        }
        self.services.picker_dirty = false;
        let choices = self.attach_choices();
        self.services.picker.update(cx, |picker, cx| {
            picker.set_items(SearchableVec::new(choices), window, cx);
            picker.set_selected_index(None, window, cx);
        });
    }

    /// Items currently offered in the sandbox's Attach autocomplete list.
    pub(super) fn attach_choices(&self) -> Vec<AttachChoice> {
        let catalog = self.service_catalog();
        let attached: Vec<&str> = match &self.services.attached {
            Some(Ok(attached)) => attached.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        };
        let spare: Vec<&Service> = catalog
            .iter()
            .filter(|service| !attached.contains(&service.provider.as_str()))
            .collect();
        let mut items: Vec<AttachChoice> = spare
            .iter()
            .map(|service| {
                let hosts: Vec<&str> = service
                    .endpoints
                    .iter()
                    .map(|endpoint| endpoint.host.as_str())
                    .collect();
                AttachChoice {
                    label: format!("{} · {}", service.provider, hosts.join(", ")).into(),
                    target: AttachTarget::Service((*service).clone()),
                }
            })
            .collect();
        for kit in &self.kits.kits.kits {
            let adds_any = kit
                .services
                .iter()
                .any(|name| spare.iter().any(|service| &service.provider == name));
            if adds_any {
                items.push(AttachChoice {
                    label: t!("Kit: %{kit}", kit = kit.name.clone())
                        .into_owned()
                        .into(),
                    target: AttachTarget::Kit(kit.name.clone()),
                });
            }
        }
        items.push(AttachChoice {
            label: t!("+ New Service…").into_owned().into(),
            target: AttachTarget::NewService,
        });
        items
    }

    /// Opens Providers' Add a Service form scoped to the chosen sandbox so the new service attaches only to it.
    pub(super) fn start_new_service_for_selected(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        self.services.pending_attach = Some(key.clone());
        self.show_page(Page::Providers, window, cx);
        if let Some(providers) = &self.providers {
            providers.update(cx, |providers, cx| {
                providers.start_adding_service_for(Some(key.name.clone()), window, cx);
            });
        }
    }

    /// Called when a service was just added from a sandbox's Services row: returns to that sandbox and attaches the
    /// new service to it (and only it).
    pub(super) fn attach_newly_added_service(
        &mut self,
        service_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.services.pending_attach.take() else {
            return;
        };
        self.show_page(Page::Sandboxes, window, cx);
        self.select(key.clone(), cx);
        let Ok(openshell) = self.openshell_for(&key.gateway) else {
            return;
        };
        let sandbox = key.name.clone();
        self.services.changing = Some(service_name.clone());
        self.services._changing = Some(cx.spawn_in(window, async move |this, cx| {
            let done = cx
                .background_spawn({
                    let sandbox = sandbox.clone();
                    let service_name = service_name.clone();
                    async move {
                        let providers = openshell.providers()?;
                        let profiles = openshell.profiles()?;
                        let service = Service::choices(&providers, &profiles)
                            .into_iter()
                            .find(|(provider, _)| provider.name == service_name)
                            .map(|(provider, profile)| Service::new(provider, profile))
                            .ok_or_else(|| {
                                OpenShellError::Run(format!(
                                    "couldn't find the newly added service “{service_name}”"
                                ))
                            })?;
                        openshell.attach_service(&sandbox, &service)?;
                        Ok::<_, OpenShellError>(())
                    }
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.services.changing = None;
                match done {
                    Ok(()) => {
                        Toast::new(
                            t!(
                                "Added and attached “%{service}” to “%{sandbox}”",
                                service = service_name,
                                sandbox = sandbox
                            )
                            .into_owned(),
                        )
                        .key("service-change")
                        .show(window, cx);
                    }
                    Err(error) => {
                        tracing::warn!(sandbox = %sandbox, "couldn't attach newly added service: {error}");
                        this.failed = Some(error.to_string());
                    }
                }
                this.look_up_services(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Attaches `service` to the chosen sandbox, or detaches it, off the window's thread.
    fn change_service(
        &mut self,
        service: Service,
        attach: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        let name = key.name.clone();
        if self.services.changing.is_some() {
            return;
        }
        let Ok(openshell) = self.openshell_for(&key.gateway) else {
            return;
        };
        self.failed = None;
        let service_name = service.provider.clone();
        self.services.changing = Some(service_name.clone());
        tracing::info!(sandbox = %name, service = %service_name, attach, "changing a service");
        self.services._changing = Some(cx.spawn_in(window, async move |this, cx| {
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
            let _ = this.update_in(cx, |this, window, cx| {
                this.services.changing = None;
                match done {
                    Ok(()) => {
                        let notice = if attach {
                            t!("Attached “%{service}”", service = service_name).into_owned()
                        } else {
                            t!("Detached “%{service}”", service = service_name).into_owned()
                        };
                        Toast::new(notice).key("service-change").show(window, cx);
                    }
                    Err(error) => {
                        tracing::warn!(sandbox = %name, "couldn't change a service: {error}");
                        this.failed = Some(error.to_string());
                    }
                }
                this.look_up_services(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Attaches every service in the kit `name` that the chosen sandbox hasn't, one after another.
    fn attach_kit(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kit) = self.kits.kits.find(name).cloned() else {
            return;
        };
        let attached = match &self.services.attached {
            Some(Ok(attached)) => attached.clone(),
            _ => return,
        };
        let missing: Vec<Service> = self
            .service_catalog()
            .into_iter()
            .filter(|service| kit.services.contains(&service.provider))
            .filter(|service| !attached.contains(&service.provider))
            .collect();
        let Some(key) = self.selected.clone() else {
            return;
        };
        let Ok(openshell) = self.openshell_for(&key.gateway) else {
            return;
        };
        let sandbox = key.name;
        if missing.is_empty() || self.services.changing.is_some() {
            return;
        }
        self.failed = None;
        let kit_name = kit.name.clone();
        self.services.changing = Some(kit_name.clone());
        self.services._changing = Some(cx.spawn_in(window, async move |this, cx| {
            let done = cx
                .background_spawn({
                    let sandbox = sandbox.clone();
                    async move {
                        missing
                            .iter()
                            .try_for_each(|service| openshell.attach_service(&sandbox, service))
                    }
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.services.changing = None;
                match done {
                    Ok(()) => {
                        Toast::new(
                            t!("Attached services from “%{kit}”", kit = kit_name).into_owned(),
                        )
                        .key("service-change")
                        .show(window, cx);
                    }
                    Err(error) => {
                        tracing::warn!(sandbox = %sandbox, "couldn't attach a kit: {error}");
                        this.failed = Some(error.to_string());
                    }
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

    /// Gives a capture stand-in attached and attachable services so the autocomplete picker can be checked visually.
    pub(super) fn give_stand_in_services(&mut self, key: &SandboxKey) {
        let profile = |id: &str, display: &str, host: &str| Profile {
            id: id.to_string(),
            display_name: display.to_string(),
            category: "service".to_string(),
            inference_capable: false,
            endpoints: vec![playpen_core::ProfileEndpoint {
                host: host.to_string(),
                port: 443,
                protocol: "rest".to_string(),
                access: "read-only".to_string(),
                enforcement: "enforce".to_string(),
            }],
            credentials: Vec::new(),
        };
        self.services.sandbox = Some(key.clone());
        self.services.attached = Some(Ok(vec!["github".to_string()]));
        self.services.catalog = Some((
            vec![
                Provider {
                    name: "github".to_string(),
                    kind: "playpen-service-github".to_string(),
                },
                Provider {
                    name: "linear".to_string(),
                    kind: "playpen-service-linear".to_string(),
                },
                Provider {
                    name: "sentry".to_string(),
                    kind: "playpen-service-sentry".to_string(),
                },
            ],
            vec![
                profile("playpen-service-github", "GitHub", "api.github.com"),
                profile("playpen-service-linear", "Linear", "api.linear.app"),
                profile("playpen-service-sentry", "Sentry", "sentry.io"),
            ],
        ));
        self.services.picker_dirty = true;
    }

    /// The chosen sandbox's Services row: each attached with a detach `×`, plus a searchable autocomplete list to
    /// attach services, kits, or create a new service scoped to this sandbox.
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
        let attach_picker = div().w(rems(13.5)).child(
            Select::new(&self.services.picker)
                .xsmall()
                .placeholder(if changing.is_some() {
                    t!("Attaching…").into_owned()
                } else {
                    t!("Attach service…").into_owned()
                })
                .search_placeholder(t!("Search services and kits…").into_owned())
                .menu_width(rems(20.))
                .disabled(!ready || changing.is_some()),
        );
        v_flex()
            .min_w_0()
            .gap_1p5()
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
                    .items_center()
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
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.change_service(service.clone(), false, window, cx);
                                })),
                        )
                    })
            }))
            .child(attach_picker)
    }
}
