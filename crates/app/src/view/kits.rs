//! Kits, a place in the sidebar: standard sets of keys, each a model key and services, made and changed here and
//! given to a sandbox in New Agent or attached to one that's running. Each change is saved as it's made.

use std::path::PathBuf;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::radio::Radio;
use playpen_core::{Kind, Kit, Kits, OpenShell, Service};

use super::sandboxes::{info_card, text_value};
use super::*;

/// The Kits place's own state.
pub(super) struct KitsPage {
    pub(super) kits: Kits,
    /// Where they're kept; none in a capture or a test, which keep them only while open.
    path: Option<PathBuf>,
    pub(super) selected: Option<String>,
    /// The chosen kit's name, as it's typed.
    name: Option<Entity<InputState>>,
    failed: Option<String>,
    _renamed: Option<Subscription>,
}

impl KitsPage {
    pub(super) fn new(watch: Watch) -> Self {
        let path = (watch == Watch::OpenShell)
            .then(|| crate::settings::file("kits.toml"))
            .flatten();
        let (kits, failed) = match &path {
            Some(path) => match Kits::load(path) {
                Ok(kits) => (kits, None),
                Err(error) => (Kits::default(), Some(error)),
            },
            None if cfg!(feature = "screenshot") => (stand_ins(), None),
            None => (Kits::default(), None),
        };
        let selected = kits.kits.first().map(|kit| kit.name.clone());
        Self {
            kits,
            path,
            selected,
            name: None,
            failed,
            _renamed: None,
        }
    }

    pub(super) fn chosen(&self) -> Option<&Kit> {
        self.kits.find(self.selected.as_deref()?)
    }

    fn save(&mut self) {
        if let Some(path) = &self.path {
            self.failed = self.kits.save(path).err();
        }
    }

    /// Changes the chosen kit and saves it.
    fn change(&mut self, change: impl FnOnce(&mut Kit)) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        if let Some(kit) = self.kits.kits.iter_mut().find(|kit| kit.name == name) {
            change(kit);
            self.save();
        }
    }
}

/// Kits for a capture to show.
fn stand_ins() -> Kits {
    Kits {
        kits: vec![
            Kit {
                name: "work".to_string(),
                model_key: Some("claude-work".to_string()),
                services: vec!["github-work".to_string(), "linear".to_string()],
            },
            Kit {
                name: "personal".to_string(),
                model_key: Some("claude-sub".to_string()),
                services: vec!["github".to_string()],
            },
        ],
    }
}

impl MainView {
    /// Asks OpenShell for every provider and profile, to say which can be a kit's model key and which its services.
    pub(super) fn look_up_catalog(&mut self, cx: &mut Context<Self>) {
        if self.watch == Watch::Nothing {
            return;
        }
        self.kits_asking = Some(cx.spawn(async move |this, cx| {
            let catalog = cx
                .background_spawn(async move {
                    let openshell = OpenShell::find().ok()?;
                    openshell.providers().ok().zip(openshell.profiles().ok())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if catalog.is_some() {
                    this.services.catalog = catalog;
                }
                cx.notify();
            });
        }));
    }

    pub(super) fn select_kit(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.kits.selected = Some(name.clone());
        let field = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(name.clone(), window, cx);
            state
        });
        // A new name takes once it's one a kit can have.
        self.kits._renamed =
            Some(
                cx.subscribe_in(&field, window, |this, field, event: &InputEvent, _, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    let typed = field.read(cx).value().trim().to_string();
                    let Some(old) = this.kits.selected.clone() else {
                        return;
                    };
                    if typed.is_empty() || typed == old || this.kits.kits.find(&typed).is_some() {
                        return;
                    }
                    this.kits.change(|kit| kit.name = typed.clone());
                    this.kits.selected = Some(typed);
                    cx.notify();
                }),
            );
        self.kits.name = Some(field);
        cx.notify();
    }

    pub(super) fn new_kit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.kits.kits.unused_name("kit");
        self.kits.kits.kits.push(Kit {
            name: name.clone(),
            ..Kit::default()
        });
        self.kits.save();
        self.show_page(Page::Kits, window, cx);
        self.select_kit(name, window, cx);
        if let Some(field) = &self.kits.name {
            field.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    fn delete_kit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.kits.selected.clone() else {
            return;
        };
        self.kits.kits.kits.retain(|kit| kit.name != name);
        self.kits.save();
        match self.kits.kits.kits.first().map(|kit| kit.name.clone()) {
            Some(first) => self.select_kit(first, window, cx),
            None => {
                self.kits.selected = None;
                self.kits.name = None;
            }
        }
        cx.notify();
    }

    /// Chooses the kit `step` places down the list from the chosen one, or up for a negative step.
    pub(super) fn select_next_kit(
        &mut self,
        step: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kits = &self.kits.kits.kits;
        if kits.is_empty() {
            return;
        }
        let at = self
            .kits
            .selected
            .as_ref()
            .and_then(|name| kits.iter().position(|kit| &kit.name == name));
        let next = match at {
            Some(at) => at.saturating_add_signed(step).min(kits.len() - 1),
            None => 0,
        };
        let name = kits[next].name.clone();
        self.select_kit(name, window, cx);
    }

    /// What a kit holds, in a line.
    fn kit_summary(kit: &Kit) -> String {
        let model = kit
            .model_key
            .clone()
            .unwrap_or_else(|| t!("no model key").into_owned());
        match kit.services.len() {
            0 => model,
            1 => t!("%{model} + 1 service", model = model).into_owned(),
            count => t!("%{model} + %{count} services", model = model, count = count).into_owned(),
        }
    }

    pub(super) fn kits_heading(&self, cx: &Context<Self>) -> Div {
        let count = self.kits.kits.kits.len();
        heading(
            t!("Kits").into_owned(),
            match count {
                0 => t!("Standard sets of keys").into_owned(),
                1 => t!("One kit").into_owned(),
                count => t!("%{count} kits", count = count).into_owned(),
            },
            cx,
        )
        .child(
            Button::new("new-kit")
                .ghost()
                .small()
                .rounded(px(999.))
                .icon(assets::IconName::Plus)
                .tooltip(t!("New Kit").into_owned())
                .on_click(cx.listener(|this, _, window, cx| this.new_kit(window, cx))),
        )
    }

    pub(super) fn kit_list(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let rows = self.kits.kits.kits.iter().map(|kit| {
            let chosen = self.kits.selected.as_ref() == Some(&kit.name);
            let name = kit.name.clone();
            list_row(chosen, cx)
                .id(ElementId::from((ElementId::from("kit"), kit.name.clone())))
                .child(kit_tile(theme.primary, rems(28. / 13.), cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_medium()
                                .child(kit.name.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(Self::kit_summary(kit)),
                        ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_kit(name.clone(), window, cx);
                }))
        });
        v_flex().size_full().child(
            v_flex()
                .id("kits")
                .flex_1()
                .overflow_y_scroll()
                .px_2()
                .pt_1()
                .gap(px(2.))
                .children(rows),
        )
    }

    pub(super) fn kit_detail(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some(kit) = self.kits.chosen() else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .p_6()
                .child(div().text_lg().font_semibold().child(t!("No kits yet").into_owned()))
                .child(
                    div()
                        .max_w(rems(28.))
                        .text_center()
                        .text_color(theme.muted_foreground)
                        .child(
                            t!(
                                "A kit is a standard set of keys, such as Work or Personal: a model key and the services that go with it, given to a sandbox together."
                            )
                            .into_owned(),
                        ),
                )
                .child(
                    Button::new("new-kit-empty")
                        .primary()
                        .small()
                        .label(t!("New Kit").into_owned())
                        .on_click(cx.listener(|this, _, window, cx| this.new_kit(window, cx))),
                )
                .into_any_element();
        };
        let catalog = self.services.catalog.as_ref();
        let known = |name: &str| {
            catalog
                .is_none_or(|(providers, _)| providers.iter().any(|provider| provider.name == name))
        };
        let missing = |name: &str| {
            div()
                .text_sm()
                .text_color(theme.warning)
                .child(t!("%{name} isn't on the gateway any more", name = name).into_owned())
        };
        let model_keys = match catalog {
            None => v_flex().child(text_value(if self.watch == Watch::Nothing {
                t!("Not asked").into_owned()
            } else {
                t!("Asking OpenShell…").into_owned()
            })),
            Some((providers, profiles)) => {
                let models: Vec<String> = providers
                    .iter()
                    .filter(|provider| {
                        Kind::of(&provider.kind).is_some()
                            || profiles.iter().any(|profile| {
                                profile.id == provider.kind && profile.is_for_a_model()
                            })
                    })
                    .map(|provider| provider.name.clone())
                    .collect();
                let chosen = kit.model_key.clone();
                let radio = |name: Option<String>, label: String| {
                    let id = ElementId::from((
                        ElementId::from("kit-model"),
                        name.clone().unwrap_or_default(),
                    ));
                    Radio::new(id)
                        .label(label)
                        .checked(chosen == name)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let name = name.clone();
                            this.kits.change(|kit| kit.model_key = name);
                            cx.notify();
                        }))
                };
                v_flex()
                    .gap_2()
                    .child(radio(None, t!("None").into_owned()))
                    .children(
                        models
                            .iter()
                            .map(|name| radio(Some(name.clone()), name.clone())),
                    )
                    .when_some(
                        kit.model_key.clone().filter(|name| !known(name)),
                        |list, name| list.child(missing(&name)),
                    )
            }
        };
        let services = match catalog {
            None => v_flex(),
            Some((providers, profiles)) => {
                let choices = Service::choices(providers, profiles);
                v_flex()
                    .gap_2()
                    .when(choices.is_empty(), |list| {
                        list.child(hint(
                            t!("None on the gateway yet: add one in Providers."),
                            cx,
                        ))
                    })
                    .children(choices.into_iter().map(|(provider, profile)| {
                        let name = provider.name.clone();
                        let hosts: Vec<&str> = profile
                            .endpoints
                            .iter()
                            .map(|endpoint| endpoint.host.as_str())
                            .collect();
                        Checkbox::new(ElementId::from((
                            ElementId::from("kit-service"),
                            name.clone(),
                        )))
                        .label(format!("{name} · {}", hosts.join(", ")))
                        .checked(kit.services.contains(&name))
                        .on_click(cx.listener(
                            move |this, checked: &bool, _, cx| {
                                let (name, checked) = (name.clone(), *checked);
                                this.kits.change(|kit| {
                                    kit.services.retain(|service| *service != name);
                                    if checked {
                                        kit.services.push(name);
                                    }
                                });
                                cx.notify();
                            },
                        ))
                    }))
                    .children(
                        kit.services
                            .iter()
                            .filter(|name| !known(name))
                            .map(|name| missing(name)),
                    )
            }
        };
        let hero = h_flex()
            .gap_3()
            .child(kit_tile(theme.primary, rems(44. / 13.), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .when_some(self.kits.name.clone(), |this, name| {
                        this.child(div().w(rems(16.)).child(small_field(&name)))
                    })
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(Self::kit_summary(kit)),
                    ),
            );
        let kit_name = kit.name.clone();
        let actions = h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                Button::new("new-sandbox-with-kit")
                    .primary()
                    .small()
                    .icon(assets::IconName::Plus)
                    .label(t!("New Sandbox with Kit…").into_owned())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_making(window, cx);
                        this.use_kit(&kit_name, cx);
                    })),
            )
            .child(
                Button::new("delete-kit")
                    .small()
                    .ghost()
                    .icon(assets::IconName::Trash)
                    .label(t!("Delete Kit").into_owned())
                    .on_click(cx.listener(|this, _, window, cx| this.delete_kit(window, cx))),
            );
        let body = v_flex()
            .gap_5()
            .p_5()
            .child(hero)
            .child(actions)
            .child(info_card(
                vec![
                    (t!("Model key").into_owned(), model_keys.items_end()),
                    (t!("Services").into_owned(), services.items_end()),
                ],
                cx,
            ))
            .when_some(self.kits.failed.clone(), |body, failed| {
                body.child(div().text_sm().text_color(theme.danger).child(failed))
            })
            .child(hint(
                t!(
                    "A kit holds the providers' names, not their keys, which stay with OpenShell. New Agent can start from one, and a running sandbox's Attach… adds a kit's services."
                ),
                cx,
            ));
        div()
            .id("kit")
            .size_full()
            .overflow_y_scroll()
            .child(body)
            .into_any_element()
    }
}

/// A kit's mark: a rounded square holding a package, in `colour`.
fn kit_tile(colour: Hsla, size: Rems, cx: &App) -> Div {
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
        .child(Icon::new(assets::IconName::Package).size(size * 0.55))
}
