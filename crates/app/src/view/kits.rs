//! Kits, a place in the sidebar: standard sets of keys, each a model key and services, made and changed here and
//! given to a sandbox in New Agent or attached to one that's running. Each change is saved as it's made.

use std::path::PathBuf;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::radio::Radio;
use playpen_core::{
    Access, EnvLine, Guess, Key, KeySource, Kind, Kit, Kits, NewProvider, NewService, OpenShell,
    Service, Variable, VariableValue, env_provider_name, find_op, guess_env, looks_secret,
    parse_env,
};

use super::sandboxes::text_value;
use super::*;

/// The Kits place's own state.
pub(super) struct KitsPage {
    pub(super) kits: Kits,
    /// Where they're kept; none in a capture or a test, which keep them only while open.
    path: Option<PathBuf>,
    pub(super) selected: Option<String>,
    /// The chosen kit's name, as it's typed while renaming.
    name: Option<Entity<InputState>>,
    /// Whether the hero title is currently in rename mode.
    renaming: bool,
    /// Inline validation note while renaming (e.g. duplicate or empty name).
    rename_problem: Option<String>,
    failed: Option<String>,
    /// The chosen kit's variables, as they're typed, each name and value field with where the value comes from.
    variables: Vec<VariableRow>,
    _variables: Vec<Subscription>,
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
            renaming: false,
            rename_problem: None,
            failed,
            import: None,
            variables: Vec::new(),
            _variables: Vec::new(),
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
                setup_folder: None,
                model_key: Some("claude-work".to_string()),
                services: vec!["github-work".to_string(), "linear".to_string()],
                variables: vec![
                    Variable {
                        name: "NODE_ENV".to_string(),
                        value: VariableValue::Value("development".to_string()),
                    },
                    Variable {
                        name: "DATABASE_URL".to_string(),
                        value: VariableValue::Reference("op://Work/Postgres/url".to_string()),
                    },
                ],
            },
            Kit {
                name: "personal".to_string(),
                setup_folder: None,
                model_key: Some("claude-sub".to_string()),
                services: vec!["github".to_string()],
                variables: Vec::new(),
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
        self.kits.renaming = false;
        self.kits.rename_problem = None;
        let field = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(name.clone(), window, cx);
            state
        });
        // A new name takes once it's one a kit can have; otherwise explain why inline.
        self.kits._renamed = Some(cx.subscribe_in(
            &field,
            window,
            |this, field, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    if this.kits.rename_problem.is_none() {
                        this.kits.renaming = false;
                        this.focus.focus(window, cx);
                        cx.notify();
                    }
                }
                InputEvent::Change => {
                    let typed = field.read(cx).value().trim().to_string();
                    let Some(old) = this.kits.selected.clone() else {
                        return;
                    };
                    if typed.is_empty() {
                        this.kits.rename_problem = Some(t!("Give the kit a name.").into_owned());
                        cx.notify();
                        return;
                    }
                    if typed == old {
                        this.kits.rename_problem = None;
                        cx.notify();
                        return;
                    }
                    if this.kits.kits.find(&typed).is_some() {
                        this.kits.rename_problem =
                            Some(t!("Another kit already has that name.").into_owned());
                        cx.notify();
                        return;
                    }
                    this.kits.rename_problem = None;
                    this.kits.change(|kit| kit.name = typed.clone());
                    this.kits.selected = Some(typed);
                    cx.notify();
                }
                _ => {}
            },
        ));
        self.kits.name = Some(field);
        let variables = self
            .kits
            .chosen()
            .map(|kit| kit.variables.clone())
            .unwrap_or_default();
        self.kits.variables.clear();
        self.kits._variables.clear();
        for variable in variables {
            self.add_variable_row(variable, window, cx);
        }
        cx.notify();
    }

    /// A row for `variable` in the chosen kit's Variables, saved again whenever it's typed in.
    fn add_variable_row(
        &mut self,
        variable: Variable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (from, value) = match variable.value {
            VariableValue::Value(value) => (From::Value, value),
            VariableValue::Reference(reference) => (From::OnePassword, reference),
            VariableValue::Keychain(service) => (From::Keychain, service),
        };
        let field =
            |text: String, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
                let placeholder = placeholder.to_string();
                cx.new(|cx| {
                    let mut state = InputState::new(window, cx).placeholder(placeholder);
                    state.set_value(text, window, cx);
                    state
                })
            };
        let name = field(variable.name, "NAME", window, cx);
        let value = field(value, "", window, cx);
        for input in [&name, &value] {
            let subscription =
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.save_variables(cx);
                    }
                });
            self.kits._variables.push(subscription);
        }
        self.kits.variables.push(VariableRow { name, value, from });
    }

    /// Writes the chosen kit's variables from their rows, and saves.
    fn save_variables(&mut self, cx: &mut Context<Self>) {
        let variables: Vec<Variable> = self
            .kits
            .variables
            .iter()
            .map(|row| {
                let text = row.value.read(cx).value().to_string();
                Variable {
                    name: row.name.read(cx).value().trim().to_string(),
                    value: match row.from {
                        From::Value => VariableValue::Value(text),
                        From::OnePassword => VariableValue::Reference(text.trim().to_string()),
                        From::Keychain => VariableValue::Keychain(text.trim().to_string()),
                    },
                }
            })
            .collect();
        self.kits.change(|kit| kit.variables = variables);
        cx.notify();
    }

    fn new_variable(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_variable_row(
            Variable {
                name: String::new(),
                value: VariableValue::Value(String::new()),
            },
            window,
            cx,
        );
        if let Some(row) = self.kits.variables.last() {
            row.name.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.save_variables(cx);
    }

    /// The chosen kit's Variables: a row each, its name, where its value comes from and the value or where to find
    /// it, with what's wrong with it under it.
    fn variables_editor(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let has_op = find_op().is_some();
        let rows = self.kits.variables.iter().enumerate().map(|(at, row)| {
            let source = |from: From, label: std::borrow::Cow<str>| {
                // An index into the rows, which are only added to or removed from as a whole.
                Radio::new(ElementId::from((ElementId::from(SharedString::from(format!("variable-from-{at}"))), label.to_string())))
                    .label(label.into_owned())
                    .checked(row.from == from)
                    .disabled(from == From::OnePassword && !has_op)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(row) = this.kits.variables.get_mut(at) {
                            row.from = from;
                        }
                        this.save_variables(cx);
                    }))
            };
            let variable = self.kits.chosen().and_then(|kit| kit.variables.get(at));
            let problem = variable.and_then(|variable| {
                if variable.name.is_empty() {
                    return None;
                }
                match variable.check() {
                    Err(problem) => Some((theme.danger, problem)),
                    Ok(()) if matches!(variable.value, VariableValue::Value(_)) && variable.looks_secret() => Some((
                        theme.warning,
                        t!("This looks like a secret, which the sandbox would hold as it is. Keep it in 1Password or the Keychain, or add it as a service so the sandbox only sees a stand-in.").into_owned(),
                    )),
                    Ok(()) => None,
                }
            });
            v_flex()
                .gap_2()
                .p_2p5()
                .rounded(theme.radius * 1.5)
                .border_1()
                .border_color(theme.border)
                .bg(shell::recessed_colour(theme))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().w(rems(11.)).child(small_field(&row.name)))
                        .child(div().flex_1().min_w_0().child(small_field(&row.value)))
                        .child(
                            Button::new(("remove-variable", at))
                                .ghost()
                                .xsmall()
                                .icon(assets::IconName::X)
                                .tooltip(t!("Remove").into_owned())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if at < this.kits.variables.len() {
                                        this.kits.variables.remove(at);
                                        // Each row has two subscriptions, its name's and its value's.
                                        this.kits._variables.drain(
                                            at * 2..(at * 2 + 2).min(this.kits._variables.len()),
                                        );
                                    }
                                    this.save_variables(cx);
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_4()
                        .child(source(From::Value, t!("Value")))
                        .child(source(From::OnePassword, t!("1Password reference")))
                        .child(source(From::Keychain, t!("Keychain item"))),
                )
                .when_some(problem, |row, (colour, problem)| {
                    row.child(div().text_xs().text_color(colour).child(problem))
                })
        });
        v_flex()
            .gap_3()
            .w_full()
            .children(rows)
            .child(
                h_flex().child(
                    Button::new("new-variable")
                        .xsmall()
                        .icon(assets::IconName::Plus)
                        .label(t!("Add Variable").into_owned())
                        .on_click(cx.listener(|this, _, window, cx| this.new_variable(window, cx))),
                ),
            )
            .child(hint(
                t!("Set in sandboxes made with the kit, not ones already running. 1Password and Keychain values are fetched as it's made, and Playpen never reads them."),
                cx,
            ))
    }

    /// Asks for the chosen kit's setup folder.
    fn choose_setup_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            let _ = this.update(cx, |this, cx| {
                this.kits
                    .change(|kit| kit.setup_folder = Some(folder.display().to_string()));
                cx.notify();
            });
        })
        .detach();
    }

    /// The chosen kit's setup folder, what Playpen does with it, and buttons to choose or clear it.
    fn setup_folder_editor(&self, kit: &Kit, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let folder = kit.setup_folder.clone();
        let installs = folder
            .as_ref()
            .is_some_and(|folder| std::path::Path::new(folder).join("install.sh").is_file());
        v_flex()
            .gap_1p5()
            .child(
                h_flex()
                    .gap_2()
                    .child(match &folder {
                        Some(folder) => h_flex()
                            .min_w_0()
                            .child(shell::code_chip(folder.clone(), cx)),
                        None => h_flex().child(hint(t!("None"), cx)),
                    })
                    .child(
                        Button::new("choose-setup-folder")
                            .xsmall()
                            .icon(assets::IconName::FolderOpen)
                            .label(t!("Choose…").into_owned())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose_setup_folder(window, cx);
                            })),
                    )
                    .when(folder.is_some(), |row| {
                        row.child(
                            Button::new("clear-setup-folder")
                                .ghost()
                                .xsmall()
                                .icon(assets::IconName::X)
                                .tooltip(t!("Clear").into_owned())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.kits.change(|kit| kit.setup_folder = None);
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .when(folder.is_some(), |list| {
                list.child(div().text_xs().text_color(theme.muted_foreground).child(if installs {
                    t!("Its install.sh runs as root over the image, on this Mac with the internet, once until the folder changes.").into_owned()
                } else {
                    t!("It has no install.sh, so it only gives its dotfiles.").into_owned()
                }))
            })
            .child(hint(
                t!("Like a dotfiles repository: an optional install.sh puts in tools however it likes, and the folder's dotfiles are linked into each new sandbox's home, apart from ones that usually hold secrets, such as .netrc and .npmrc."),
                cx,
            ))
    }

    pub(super) fn new_kit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.kits.kits.unused_name("kit");
        self.kits.kits.kits.push(Kit {
            name: name.clone(),
            setup_folder: None,
            ..Kit::default()
        });
        self.kits.save();
        self.show_page(Page::Kits, window, cx);
        self.select_kit(name, window, cx);
        self.kits.renaming = true;
        if let Some(field) = &self.kits.name {
            field.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn delete_kit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.kits.selected.clone() else {
            return;
        };
        let Some(pos) = self.kits.kits.kits.iter().position(|kit| kit.name == name) else {
            return;
        };
        let removed = self.kits.kits.kits.remove(pos);
        self.kits.save();
        match self.kits.kits.kits.first().map(|kit| kit.name.clone()) {
            Some(first) => self.select_kit(first, window, cx),
            None => {
                self.kits.selected = None;
                self.kits.name = None;
                self.kits.renaming = false;
            }
        }
        let view = cx.entity().downgrade();
        Toast::new(t!("Deleted kit “%{name}”", name = removed.name.clone()).into_owned())
            .key("kit-deleted")
            .undo(move |window, cx| {
                let kit = removed.clone();
                let _ = view.update(cx, |this, cx| {
                    if this.kits.kits.find(&kit.name).is_none() {
                        let at = pos.min(this.kits.kits.kits.len());
                        let restored_name = kit.name.clone();
                        this.kits.kits.kits.insert(at, kit);
                        this.kits.save();
                        this.select_kit(restored_name, window, cx);
                    }
                });
            })
            .show(window, cx);
        cx.notify();
    }

    pub(super) fn cancel_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .kits
            .import
            .as_ref()
            .is_some_and(|import| import.saving)
        {
            return;
        }
        self.kits.import = None;
        self.focus.focus(window, cx);
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
                    div()
                        .child(
                            Button::new("new-kit")
                                .ghost()
                                .small()
                                .rounded(px(999.))
                                .icon(assets::IconName::Plus)
                                .tooltip(t!("New Kit").into_owned())
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.new_kit(window, cx)),
                                ),
                        )
                        .tour_target("new-kit"),
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
            None => v_flex().child(text_value(if self.watch == Watch::Nothing {
                t!("Not asked").into_owned()
            } else {
                t!("Asking OpenShell…").into_owned()
            })),
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
            .items_center()
            .child(kit_tile(theme.primary, rems(42. / 13.), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(if self.kits.renaming {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .when_some(self.kits.name.clone(), |row, name| {
                                row.child(div().w(rems(16.)).child(small_field(&name)))
                            })
                            .child(
                                Button::new("done-rename-kit")
                                    .ghost()
                                    .xsmall()
                                    .disabled(self.kits.rename_problem.is_some())
                                    .label(t!("Done").into_owned())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.kits.renaming = false;
                                        this.focus.focus(window, cx);
                                        cx.notify();
                                    })),
                            )
                    } else {
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_lg()
                                    .font_semibold()
                                    .child(kit.name.clone()),
                            )
                            .child(
                                Button::new("rename-kit")
                                    .ghost()
                                    .xsmall()
                                    .icon(assets::IconName::Pencil)
                                    .tooltip(t!("Rename Kit").into_owned())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.kits.renaming = true;
                                        if let Some(field) = &this.kits.name {
                                            field.read(cx).focus_handle(cx).focus(window, cx);
                                        }
                                        cx.notify();
                                    })),
                            )
                    })
                    .when_some(self.kits.rename_problem.clone(), |col, problem| {
                        col.child(div().text_xs().text_color(theme.danger).child(problem))
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
                    .outline()
                    .icon(assets::IconName::Trash)
                    .label(t!("Delete Kit").into_owned())
                    .on_click(cx.listener(|this, _, window, cx| this.delete_kit(window, cx))),
            );
        let pinned_header = v_flex()
            .flex_none()
            .gap_3()
            .px_5()
            .py_4()
            .border_b_1()
            .border_color(shell::hairline(theme))
            .child(hero)
            .child(actions);
        let recessed_box = |content: Div| {
            div()
                .p_3()
                .rounded(theme.radius * 1.5)
                .border_1()
                .border_color(theme.border)
                .bg(shell::recessed_colour(theme))
                .child(content)
        };
        let body = v_flex()
            .gap_5()
            .p_5()
            .child(
                section(t!("Model key"), cx)
                    .child(recessed_box(model_keys))
                    .tour_target("kit-model"),
            )
            .child(section(t!("Services"), cx).child(recessed_box(services)))
            .child(section(t!("Setup folder"), cx).child(self.setup_folder_editor(kit, cx)))
            .child(section(t!("Variables"), cx).child(self.variables_editor(cx)))
            .when_some(self.kits.failed.clone(), |body, failed| {
                body.child(div().text_sm().text_color(theme.danger).child(failed))
            })
            .child(hint(
                t!(
                    "A kit holds the providers' names, not their keys, which stay with OpenShell. New Agent can start from one, and a running sandbox's Attach… adds a kit's services."
                ),
                cx,
            ));
        v_flex()
            .size_full()
            .child(pinned_header)
            .child(div().id("kit").flex_1().overflow_y_scroll().child(body))
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

/// A variable being edited: its name and value fields, and where the value comes from.
struct VariableRow {
    name: Entity<InputState>,
    value: Entity<InputState>,
    from: From,
}

/// Where a variable's value comes from, as the editor offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum From {
    Value,
    OnePassword,
    Keychain,
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
    /// Whether it's set in the sandbox as it is, rather than added as a key: for what isn't a secret.
    as_variable: bool,
}

/// What became of each variable, by provider name, and what went wrong with any.
struct Imported {
    model_key: Option<String>,
    services: Vec<String>,
    variables: Vec<Variable>,
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
                // What Playpen doesn't know and doesn't look like a secret is a plain variable, such as NODE_ENV.
                let as_variable = guess == Guess::Unknown && !looks_secret(&line.variable);
                ImportRow {
                    include: guess != Guess::Unknown || as_variable,
                    as_variable,
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
        // The ticked lines kept as they are, which go only in the kit.
        let variables: Vec<Variable> = import
            .rows
            .iter()
            .filter(|row| row.include && row.as_variable)
            .map(|row| Variable {
                name: row.line.variable.clone(),
                value: if row.line.is_reference() {
                    VariableValue::Reference(row.line.value.clone())
                } else {
                    VariableValue::Value(row.line.value.clone())
                },
            })
            .collect();
        // Each other ticked line as what to add: its name, and a model kind or a host.
        let mut wanted = Vec::new();
        for row in import
            .rows
            .iter()
            .filter(|row| row.include && !row.as_variable)
        {
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
                variables,
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
                            setup_folder: None,
                            model_key: imported.model_key,
                            services: imported.services,
                            variables: imported.variables,
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
        let short_from = {
            let file = import
                .from
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            match import
                .from
                .parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().into_owned())
            {
                Some(parent) if !parent.is_empty() => format!("{parent}/{file}"),
                _ => file,
            }
        };
        let rows = import.rows.iter().enumerate().map(|(at, row)| {
            let what = match &row.guess {
                Guess::Model(kind) => t!("%{kind} model key", kind = kind.name()).into_owned(),
                _ if row.as_variable => t!("Plain variable").into_owned(),
                Guess::Service(_) => t!("Service").into_owned(),
                Guess::Unknown => t!("Needs a host").into_owned(),
            };
            let source_badge = if row.line.is_reference() {
                t!("1Password").into_owned()
            } else if row.as_variable {
                t!("Value").into_owned()
            } else {
                t!("Secret").into_owned()
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
                .gap_2p5()
                .p_3()
                .rounded(theme.radius * 1.5)
                .border_1()
                .border_color(theme.border)
                .bg(shell::recessed_colour(theme))
                .child(
                    h_flex()
                        .gap_2p5()
                        .items_center()
                        .child(
                            Checkbox::new(("import-row", at))
                                .checked(row.include)
                                .disabled(import.saving)
                                .on_click(toggle(|row, checked| row.include = checked)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .child(shell::code_chip(row.line.variable.clone(), cx)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(what),
                        )
                        .child(
                            shell::badge(Tag::secondary())
                                .xsmall()
                                .child(source_badge),
                        ),
                )
                .when(row.include, |card| {
                    card.child(
                        v_flex()
                            .gap_2()
                            .pl_6()
                            .when(!row.as_variable, |details| {
                                details.child(
                                    h_flex()
                                        .flex_wrap()
                                        .gap_3()
                                        .items_end()
                                        .child(
                                            v_flex()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.muted_foreground)
                                                        .child(t!("Name").into_owned()),
                                                )
                                                .child(
                                                    div()
                                                        .w(rems(11.))
                                                        .child(small_field(&row.name)),
                                                ),
                                        )
                                        .when(!matches!(row.guess, Guess::Model(_)), |line| {
                                            line.child(
                                                v_flex()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(theme.muted_foreground)
                                                            .child(t!("Host").into_owned()),
                                                    )
                                                    .child(
                                                        div()
                                                            .w(rems(14.))
                                                            .child(small_field(&row.host)),
                                                    ),
                                            )
                                        }),
                                )
                            })
                            .when(!matches!(row.guess, Guess::Model(_)), |details| {
                                details.child(
                                    h_flex()
                                        .flex_wrap()
                                        .gap_4()
                                        .child(
                                            Checkbox::new(("import-variable", at))
                                                .label(t!("Set as a variable").into_owned())
                                                .checked(row.as_variable)
                                                .disabled(import.saving)
                                                .on_click(toggle(|row, checked| {
                                                    row.as_variable = checked
                                                })),
                                        )
                                        .when(!row.as_variable, |line| {
                                            line.child(
                                                Checkbox::new(("import-writes", at))
                                                    .label(t!("Allow writes").into_owned())
                                                    .checked(row.writes)
                                                    .disabled(import.saving)
                                                    .on_click(toggle(|row, checked| {
                                                        row.writes = checked
                                                    })),
                                            )
                                        }),
                                )
                            }),
                    )
                    .when(!row.as_variable && there.contains(&name), |card| {
                        card.child(div().pl_6().child(hint(
                            t!("OpenShell has a provider called that already, so the kit uses it as it is."),
                            cx,
                        )))
                    })
                })
        });
        let body = v_flex()
            .gap_4()
            .p_4()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .items_center()
                    .child(div().text_sm().text_color(theme.muted_foreground).child(t!("From").into_owned()))
                    .child(shell::code_chip(short_from, cx)),
            )
            .child(hint(
                t!("Give each service its host. Unrecognized keys start unticked; 1Password references (op://…) go directly from 1Password to OpenShell."),
                cx,
            ))
            .child(
                section(t!("Kit"), cx).child(div().w(rems(16.)).child(small_field(&import.kit_name))),
            )
            .children(rows);
        let footer_left = match &self.kits.failed {
            Some(failed) => div()
                .text_sm()
                .text_color(theme.danger)
                .child(failed.clone())
                .into_any_element(),
            None => div().into_any_element(),
        };
        let cancel = Button::new("cancel-import")
            .ghost()
            .small()
            .disabled(import.saving)
            .label(t!("Cancel").into_owned())
            .on_click(cx.listener(|this, _, window, cx| this.cancel_import(window, cx)));
        let primary = Button::new("finish-import")
            .primary()
            .small()
            .loading(import.saving)
            .disabled(!import.rows.iter().any(|row| row.include))
            .label(t!("Add and Make Kit").into_owned())
            .on_click(cx.listener(|this, _, _, cx| this.finish_import(cx)));
        self.form_panel(
            t!("New Kit from .env"),
            "import",
            body,
            footer_left,
            cancel,
            primary,
            import.saving,
            |this, _, window, cx| this.cancel_import(window, cx),
            cx,
        )
    }
}
