//! Kits, a place in the sidebar: standard sets of keys, each a model key and services, made and changed here and
//! given to a sandbox in New Agent or attached to one that's running. Each change is saved as it's made.

use std::path::PathBuf;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::radio::Radio;
use playpen_core::{
    Access, EnvLine, Guess, Key, KeySource, Kind, Kit, Kits, NewProvider, NewService, OpenShell,
    Service, env_provider_name, find_op, guess_env, parse_env,
};

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
    /// A kit being made from a `.env` file, shown in place of the chosen kit.
    pub(super) import: Option<EnvImport>,
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
            import: None,
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
            h_flex()
                .flex_none()
                .child(
                    Button::new("new-kit-from-env")
                        .ghost()
                        .small()
                        .rounded(px(999.))
                        .icon(assets::IconName::FileInput)
                        .tooltip(t!("New Kit from .env…").into_owned())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.choose_env_file(window, cx)),
                        ),
                )
                .child(
                    Button::new("new-kit")
                        .ghost()
                        .small()
                        .rounded(px(999.))
                        .icon(assets::IconName::Plus)
                        .tooltip(t!("New Kit").into_owned())
                        .on_click(cx.listener(|this, _, window, cx| this.new_kit(window, cx))),
                ),
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
        if let Some(import) = &self.kits.import {
            return self.import_panel(import, cx);
        }
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

/// A kit being made from a `.env` file: each variable, what Playpen takes it for, and whether it goes in.
pub(super) struct EnvImport {
    from: PathBuf,
    kit_name: Entity<InputState>,
    rows: Vec<ImportRow>,
    saving: bool,
    _saving: Option<Task<()>>,
}

struct ImportRow {
    line: EnvLine,
    guess: Guess,
    include: bool,
    name: Entity<InputState>,
    host: Entity<InputState>,
    writes: bool,
}

/// What became of each variable, by provider name, and what went wrong with any.
struct Imported {
    model_key: Option<String>,
    services: Vec<String>,
    failures: Vec<String>,
}

impl MainView {
    /// The kit's name and each line's variable, whether it's ticked, its name and its host, for tests.
    #[cfg(test)]
    pub(super) fn import_rows(&self, cx: &App) -> (String, Vec<(String, bool, String, String)>) {
        let import = self
            .kits
            .import
            .as_ref()
            .expect("a kit is being made from a .env file");
        (
            import.kit_name.read(cx).value().to_string(),
            import
                .rows
                .iter()
                .map(|row| {
                    (
                        row.line.variable.clone(),
                        row.include,
                        row.name.read(cx).value().to_string(),
                        row.host.read(cx).value().to_string(),
                    )
                })
                .collect(),
        )
    }

    /// Asks for a `.env` file and shows what a kit made from it would hold.
    pub(super) fn choose_env_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("Read").into_owned().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| this.read_env_file(path, window, cx));
        })
        .detach();
    }

    pub(super) fn read_env_file(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                self.kits.failed = Some(
                    t!(
                        "Couldn't read %{path}: %{error}",
                        path = path.display(),
                        error = error.to_string()
                    )
                    .into_owned(),
                );
                cx.notify();
                return;
            }
        };
        let field = |value: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx);
                state.set_value(value, window, cx);
                state
            })
        };
        let rows = parse_env(&text)
            .into_iter()
            .map(|line| {
                let guess = guess_env(&line.variable);
                let host = match guess {
                    Guess::Service(host) => host.to_string(),
                    Guess::Model(_) | Guess::Unknown => String::new(),
                };
                ImportRow {
                    include: guess != Guess::Unknown,
                    name: field(env_provider_name(&line.variable), window, cx),
                    host: field(host, window, cx),
                    writes: false,
                    guess,
                    line,
                }
            })
            .collect();
        // Named after the folder the file's in, as the project it's for.
        let base = path
            .parent()
            .and_then(|folder| folder.file_name())
            .map(|name| env_provider_name(&name.to_string_lossy()))
            .unwrap_or_else(|| "kit".to_string());
        let kit_name = self.kits.kits.unused_name(&base);
        self.kits.import = Some(EnvImport {
            from: path,
            kit_name: field(kit_name, window, cx),
            rows,
            saving: false,
            _saving: None,
        });
        self.kits.failed = None;
        self.show_page(Page::Kits, window, cx);
        self.look_up_catalog(cx);
        cx.notify();
    }

    /// Adds every variable ticked as a model key or a service, leaving any whose name OpenShell has already as it
    /// is, then makes the kit of them.
    fn finish_import(&mut self, cx: &mut Context<Self>) {
        let Some(import) = &mut self.kits.import else {
            return;
        };
        if import.saving {
            return;
        }
        let kit_name = import.kit_name.read(cx).value().trim().to_string();
        if kit_name.is_empty() || self.kits.kits.find(&kit_name).is_some() {
            self.kits.failed = Some(t!("Give the kit a name no other kit has.").into_owned());
            cx.notify();
            return;
        }
        // Each ticked line as what to add: its name, and a model kind or a host.
        let mut wanted = Vec::new();
        for row in import.rows.iter().filter(|row| row.include) {
            let name = row.name.read(cx).value().trim().to_string();
            let host = row.host.read(cx).value().trim().to_string();
            let source = if row.line.is_reference() {
                KeySource::Reference(row.line.value.clone())
            } else {
                KeySource::Key(Key::new(&row.line.value))
            };
            let access = if row.writes {
                Access::ReadWrite
            } else {
                Access::ReadOnly
            };
            wanted.push((
                row.line.variable.clone(),
                row.guess.clone(),
                name,
                host,
                access,
                source,
            ));
        }
        import.saving = true;
        self.kits.failed = None;
        let done = cx.background_spawn(async move {
            let openshell = OpenShell::find().map_err(|error| error.to_string())?;
            let there: Vec<String> = openshell
                .providers()
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|provider| provider.name)
                .collect();
            let op = find_op();
            let mut imported = Imported {
                model_key: None,
                services: Vec::new(),
                failures: Vec::new(),
            };
            for (variable, guess, name, host, access, source) in wanted {
                let is_model = matches!(guess, Guess::Model(_));
                let added = if there.contains(&name) {
                    Ok(())
                } else {
                    match guess {
                        Guess::Model(kind) => NewProvider::new(kind, &name, "")
                            .map_err(|error| error.to_string())
                            .and_then(|new| {
                                openshell
                                    .add_provider_from(&new, &source, op.as_deref())
                                    .map_err(|error| error.to_string())
                            }),
                        Guess::Service(_) | Guess::Unknown => {
                            NewService::new(&name, &host, &variable, access)
                                .map_err(|error| error.to_string())
                                .and_then(|new| {
                                    openshell
                                        .add_service(&new, &source, op.as_deref())
                                        .map_err(|error| error.to_string())
                                })
                        }
                    }
                };
                match added {
                    Ok(()) if is_model && imported.model_key.is_none() => {
                        imported.model_key = Some(name);
                    }
                    Ok(()) if is_model => {}
                    Ok(()) => imported.services.push(name),
                    Err(error) => imported.failures.push(format!("{variable}: {error}")),
                }
            }
            Ok::<_, String>(imported)
        });
        let task = cx.spawn(async move |this, cx| {
            let done = done.await;
            let _ = this.update(cx, |this, cx| {
                match done {
                    Ok(imported) => {
                        this.kits.kits.kits.push(Kit {
                            name: kit_name.clone(),
                            model_key: imported.model_key,
                            services: imported.services,
                        });
                        this.kits.save();
                        this.kits.import = None;
                        this.kits.selected = Some(kit_name.clone());
                        this.kits.name = None;
                        if !imported.failures.is_empty() {
                            this.kits.failed = Some(imported.failures.join("\n"));
                        }
                        tracing::info!(kit = %kit_name, "made a kit from a .env file");
                    }
                    Err(error) => {
                        if let Some(import) = &mut this.kits.import {
                            import.saving = false;
                        }
                        this.kits.failed = Some(error);
                    }
                }
                this.look_up_catalog(cx);
                cx.notify();
            });
        });
        if let Some(import) = &mut self.kits.import {
            import._saving = Some(task);
        }
        cx.notify();
    }

    /// What a kit made from a `.env` file would hold, each line to tick, name and point at a host.
    pub(super) fn import_panel(&self, import: &EnvImport, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let header = Self::panel_header(t!("New Kit from .env"), cx).child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("cancel-import")
                        .ghost()
                        .xsmall()
                        .disabled(import.saving)
                        .label(t!("Cancel").into_owned())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.kits.import = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("finish-import")
                        .primary()
                        .xsmall()
                        .loading(import.saving)
                        .disabled(!import.rows.iter().any(|row| row.include))
                        .label(t!("Add and Make Kit").into_owned())
                        .on_click(cx.listener(|this, _, _, cx| this.finish_import(cx))),
                ),
        );
        let there: Vec<String> = self
            .services
            .catalog
            .as_ref()
            .map(|(providers, _)| {
                providers
                    .iter()
                    .map(|provider| provider.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let rows = import.rows.iter().enumerate().map(|(at, row)| {
            let what = match &row.guess {
                Guess::Model(kind) => t!("%{kind} model key", kind = kind.name()).into_owned(),
                Guess::Service(_) => t!("Service").into_owned(),
                Guess::Unknown => t!("Service, if you say where it goes").into_owned(),
            };
            let value = if row.line.is_reference() {
                row.line.value.clone()
            } else {
                t!("a key, kept by OpenShell").into_owned()
            };
            let name = row.name.read(cx).value().trim().to_string();
            let toggle = move |change: fn(&mut ImportRow, bool)| {
                cx.listener(move |this: &mut MainView, checked: &bool, _, cx| {
                    if let Some(import) = &mut this.kits.import
                        && let Some(row) = import.rows.get_mut(at)
                    {
                        change(row, *checked);
                        cx.notify();
                    }
                })
            };
            // An index into a list that's only ever read in this order, since a file's lines don't move.
            v_flex()
                .gap_2()
                .p_3()
                .rounded(theme.radius * 1.5)
                .border_1()
                .border_color(theme.border)
                .bg(shell::recessed_colour(theme))
                .child(
                    h_flex()
                        .gap_3()
                        .child(
                            Checkbox::new(("import-row", at))
                                .checked(row.include)
                                .disabled(import.saving)
                                .on_click(toggle(|row, checked| row.include = checked)),
                        )
                        .child(h_flex().flex_none().child(shell::code_chip(row.line.variable.clone(), cx)))
                        .child(div().flex_1().min_w_0().truncate().text_sm().text_color(theme.muted_foreground).child(what))
                        .child(div().min_w_0().truncate().text_xs().text_color(theme.muted_foreground).child(value)),
                )
                .when(row.include, |card| {
                    card.child(
                        h_flex()
                            .flex_wrap()
                            .gap_3()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(theme.muted_foreground).child(t!("Name").into_owned()))
                                    .child(div().w(rems(11.)).child(small_field(&row.name))),
                            )
                            .when(!matches!(row.guess, Guess::Model(_)), |line| {
                                line.child(
                                    v_flex()
                                        .gap_1()
                                        .child(div().text_xs().text_color(theme.muted_foreground).child(t!("Host").into_owned()))
                                        .child(div().w(rems(14.)).child(small_field(&row.host))),
                                )
                                .child(
                                    Checkbox::new(("import-writes", at))
                                        .label(t!("Allow writes").into_owned())
                                        .checked(row.writes)
                                        .disabled(import.saving)
                                        .on_click(toggle(|row, checked| row.writes = checked)),
                                )
                            }),
                    )
                    .when(there.contains(&name), |card| {
                        card.child(hint(
                            t!("OpenShell has a provider called that already, so the kit uses it as it is."),
                            cx,
                        ))
                    })
                })
        });
        let body = v_flex()
            .gap_4()
            .p_4()
            .child(hint(
                t!("From %{path}. A .env says what a key's called but not where it goes, so give each service its host; one Playpen doesn't know starts unticked.", path = import.from.display()),
                cx,
            ))
            .child(section(t!("Kit"), cx).child(div().w(rems(16.)).child(small_field(&import.kit_name))))
            .children(rows)
            .when_some(self.kits.failed.clone(), |body, failed| {
                body.child(div().text_sm().text_color(theme.danger).child(failed))
            })
            .child(hint(
                t!("Values that are 1Password references, op://…, go from op to OpenShell; Playpen never reads them."),
                cx,
            ));
        v_flex()
            .size_full()
            .child(header)
            .child(div().id("import").flex_1().overflow_y_scroll().child(body))
            .into_any_element()
    }
}
