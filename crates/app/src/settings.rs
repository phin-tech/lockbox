//! The Settings window (⌘,) and the settings behind it, which `playpen_core::Settings` keeps in
//! `~/Library/Application Support/tech.phin.playpen/settings.toml`. `PLAYPEN_SETTINGS` points somewhere else.
//! The window is Typecase's frame, a sidebar of sections; its Appearance section is Typecase's theme picker.

use std::path::{Path, PathBuf};

use gpui_kit::component::radio::Radio;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, StyledExt as _, button::Button, h_flex, switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use playpen_core::{Installed, Settings, ShowIn, step_text_scale};
use rust_i18n::t;
use typecase::settings::{Section, SettingsFrame};

use crate::login_item;
use crate::shortcuts::RunAction;
use crate::update;
use crate::{commands, theme};

/// The settings, where they're kept, and the Settings window if it's open.
struct Prefs {
    settings: Settings,
    path: Option<PathBuf>,
    window: Option<AnyWindowHandle>,
}

impl Global for Prefs {}

/// A file the app keeps between runs, `name` in Application Support: `settings.toml`, `state.toml`. They go beside
/// `PLAYPEN_SETTINGS` when that's set. Captures keep none otherwise, so a frame never depends on, or changes,
/// what's kept.
pub fn file(name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PLAYPEN_SETTINGS") {
        let path = PathBuf::from(path);
        return Some(match name {
            "settings.toml" => path,
            _ => path.with_file_name(name),
        });
    }
    if cfg!(feature = "screenshot") {
        return None;
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(
        home.join("Library/Application Support/tech.phin.playpen")
            .join(name),
    )
}

/// Reads the settings and shows the theme they choose. After `theme::init`, before any window opens.
pub fn init(cx: &mut App) {
    init_at(file("settings.toml"), cx);
}

/// [`init`], with the settings kept at `path`, or nowhere.
fn init_at(path: Option<PathBuf>, cx: &mut App) {
    let settings = path.as_deref().map(Settings::load).unwrap_or_default();
    update::set_beta(settings.beta_updates);
    theme::set_text_scale(settings.text_scale, cx);
    typecase::theme::apply(theme::from_settings(&settings), cx);
    cx.set_global(Prefs {
        settings,
        path,
        window: None,
    });
    // The picker in Settings and View ▸ Dark Mode change the theme through Typecase, which says so here.
    typecase::theme::on_change(cx, |shown, cx| {
        change(cx, |settings| theme::into_settings(shown, settings))
    });
}

/// The settings as they stand, or the defaults before [`init`].
pub fn current(cx: &App) -> Settings {
    cx.try_global::<Prefs>()
        .map(|prefs| prefs.settings.clone())
        .unwrap_or_default()
}

/// Changes the settings, saves them, and redraws every window with them.
fn change(cx: &mut App, edit: impl FnOnce(&mut Settings)) {
    let mut settings = current(cx);
    edit(&mut settings);
    let Some(prefs) = cx.try_global::<Prefs>() else {
        return;
    };
    if prefs.settings == settings {
        return;
    }
    let prefs = cx.global_mut::<Prefs>();
    prefs.settings = settings;
    if let Some(path) = &prefs.path {
        save(&prefs.settings, path);
    }
    cx.refresh_windows();
}

fn save(settings: &Settings, path: &Path) {
    if let Err(error) = settings.save(path) {
        tracing::warn!("settings not saved to {}: {error}", path.display());
    }
}

pub fn set_beta_updates(on: bool, cx: &mut App) {
    update::set_beta(on);
    change(cx, |settings| settings.beta_updates = on);
}

/// Makes text a step bigger (1), smaller (-1), or with 0, its actual size.
pub fn step_text(steps: isize, cx: &mut App) {
    change(cx, |settings| {
        settings.text_scale = if steps == 0 {
            1.0
        } else {
            step_text_scale(settings.text_scale, steps)
        };
    });
    theme::set_text_scale(current(cx).text_scale, cx);
    cx.refresh_windows();
}

/// Opens the Settings window, or brings it forward if it's open.
pub fn open(cx: &mut App) {
    let open = cx.try_global::<Prefs>().and_then(|prefs| prefs.window);
    if let Some(handle) = open
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let bounds = Bounds::centered(None, size(px(760.), px(540.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(560.), px(360.))),
        is_minimizable: false,
        titlebar: Some(TitlebarOptions {
            title: Some(t!("%{app} Settings", app = "Playpen").into_owned().into()),
            ..Default::default()
        }),
        show: !cfg!(feature = "screenshot"),
        focus: !cfg!(feature = "screenshot"),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| SettingsWindow::new(window, cx))
    }) {
        Ok((handle, _)) => {
            if cx.has_global::<Prefs>() {
                cx.global_mut::<Prefs>().window = Some(handle);
            }
        }
        Err(error) => tracing::error!("the Settings window didn't open: {error}"),
    }
}

/// The sections, in the sidebar's order.
fn sections() -> Vec<Section> {
    vec![
        Section::new(t!("General").into_owned(), |_, cx| general(cx)).keywords([
            "text", "size", "herdr", "ghostty", "terminal", "sessions", "agents",
        ]),
        Section::appearance(),
        Section::new(t!("Updates").into_owned(), |_, cx| updates(cx)).keywords(["beta", "sparkle"]),
    ]
}

/// The window's root: Typecase's frame, with the app's Commands, so ⌘W and ⌘Q work here too.
pub struct SettingsWindow {
    frame: Entity<SettingsFrame>,
    focus: FocusHandle,
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let frame = cx.new(|cx| {
            #[cfg_attr(
                not(feature = "screenshot"),
                expect(unused_mut, reason = "only shots change it")
            )]
            let mut frame = SettingsFrame::new(sections(), window, cx);
            // A shot can't click in this window, since its keys go to the main one, so it names the section to show.
            #[cfg(feature = "screenshot")]
            if let Ok(section) = std::env::var("PLAYPEN_SETTINGS_SECTION") {
                frame.show(&section, cx);
            }
            frame
        });
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self { frame, focus }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus)
            .key_context("Settings")
            .on_action(cx.listener(|_, action: &RunAction, window, cx| {
                commands::run(action.name, window, cx);
            }))
            .child(self.frame.clone())
    }
}

/// A titled part of a section.
fn part(title: std::borrow::Cow<str>, body: impl IntoElement) -> impl IntoElement {
    v_flex()
        .gap_2()
        .child(div().text_sm().font_semibold().child(title.into_owned()))
        .child(body)
}

fn text_button(id: &'static str, label: std::borrow::Cow<str>, steps: isize) -> Button {
    Button::new(id)
        .outline()
        .label(label.into_owned())
        .on_click(move |_, _, cx| step_text(steps, cx))
}

fn general(cx: &mut App) -> Div {
    let muted = cx.theme().muted_foreground;
    let settings = current(cx);
    v_flex()
        .p_4()
        .gap_6()
        .child(part(
            t!("Startup"),
            v_flex()
                .gap_2()
                .child(
                    Switch::new("login")
                        .label(t!("Open %{app} at login", app = "Playpen").into_owned())
                        .checked(login_item::is_on())
                        .on_click(|on, window, _| {
                            if let Err(error) = login_item::set(*on) {
                                tracing::warn!("open at login not changed: {error:#}");
                            }
                            window.refresh();
                        }),
                )
                .when(login_item::needs_approval(), |this| {
                    this.child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(muted)
                                    .child(t!("macOS asks you to allow it first.").into_owned()),
                            )
                            .child(
                                Button::new("login-items")
                                    .outline()
                                    .label(t!("Open Login Items").into_owned())
                                    .on_click(|_, _, _| login_item::open_system_settings()),
                            ),
                    )
                }),
        ))
        .child(part(t!("Show Sessions In"), sessions(&settings, cx)))
        .child(part(
            t!("Text Size"),
            h_flex()
                .gap_2()
                .items_center()
                .child(text_button("smaller", t!("Smaller"), -1))
                .child(text_button("actual", t!("Actual Size"), 0))
                .child(text_button("bigger", t!("Bigger"), 1))
                .child(
                    div()
                        .pl_2()
                        .text_sm()
                        .text_color(muted)
                        .child(format!("{:.0}%", settings.text_scale * 100.)),
                ),
        ))
}

/// Where sessions are shown unless one is opened somewhere else, and whether herdr-core keeps them. What's installed
/// is looked for each time this is drawn, so installing something while Settings is open shows at once.
fn sessions(settings: &Settings, cx: &mut App) -> Div {
    let muted = cx.theme().muted_foreground;
    let installed = Installed::now();
    let missing = |there: bool, said: std::borrow::Cow<str>| {
        if there {
            said.into_owned()
        } else {
            t!("Not installed: sessions open in Terminal until it is").into_owned()
        }
    };
    let choice = |id: &'static str, show_in: ShowIn, label: std::borrow::Cow<str>, said: String| {
        h_flex()
            .gap_2()
            .child(
                Radio::new(id)
                    .label(label.into_owned())
                    .checked(settings.show_in == show_in)
                    .on_click(move |_, _, cx| change(cx, |settings| settings.show_in = show_in)),
            )
            .child(div().text_sm().text_color(muted).child(said))
    };
    let keeps = if installed.herdr_core {
        t!(
            "Closing a window only detaches; the session goes on, and Playpen sees what its agent is doing"
        )
    } else {
        t!(
            "herdr-core isn't installed, so a window is the session, and closing it ends the session"
        )
    };
    v_flex()
        .gap_2()
        .child(choice(
            "show-automatic",
            ShowIn::Automatic,
            t!("Automatic"),
            t!("The Herdr app when it's installed, Terminal otherwise").into_owned(),
        ))
        .child(choice(
            "show-herdr",
            ShowIn::HerdrApp,
            t!("Herdr App"),
            missing(
                installed.herdr_app,
                t!("A tab for each session, in a Playpen space"),
            ),
        ))
        .child(choice(
            "show-ghostty",
            ShowIn::Ghostty,
            t!("Ghostty"),
            missing(
                installed.ghostty,
                t!("A Ghostty of its own for each session"),
            ),
        ))
        .child(choice(
            "show-terminal",
            ShowIn::Terminal,
            t!("Terminal"),
            t!("A Terminal window for each session").into_owned(),
        ))
        .child(
            v_flex()
                .pt_2()
                .gap_1()
                .child(
                    Switch::new("keep-sessions")
                        .label(t!("Keep sessions in herdr-core").into_owned())
                        // The Herdr app always has herdr-core keep its sessions, so the switch is moot there.
                        .checked(settings.keep_sessions || settings.show_in == ShowIn::HerdrApp)
                        .disabled(!installed.herdr_core || settings.show_in == ShowIn::HerdrApp)
                        .on_click(|on, _, cx| change(cx, |settings| settings.keep_sessions = *on)),
                )
                .child(div().text_sm().text_color(muted).child(keeps.into_owned())),
        )
}

fn updates(cx: &mut App) -> Div {
    let muted = cx.theme().muted_foreground;
    let settings = current(cx);
    let sparkle = update::available();
    v_flex()
        .p_4()
        .gap_3()
        .child(
            Switch::new("automatic")
                .label(t!("Check for updates automatically").into_owned())
                .checked(update::automatic())
                .disabled(!sparkle)
                .on_click(|on, window, _| {
                    update::set_automatic(*on);
                    window.refresh();
                }),
        )
        .child(
            Switch::new("beta")
                .label(t!("Get beta versions, which come before a release").into_owned())
                .checked(settings.beta_updates)
                .on_click(|on, _, cx| set_beta_updates(*on, cx)),
        )
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    Button::new("check")
                        .outline()
                        .label(t!("Check Now").into_owned())
                        .disabled(!sparkle)
                        .on_click(|_, _, _| {
                            update::check();
                        }),
                )
                .child(
                    div().text_sm().text_color(muted).child(
                        if sparkle {
                            t!(
                                "Once a day, %{app} looks for a newer release on apps.phin.tech.",
                                app = "Playpen"
                            )
                        } else {
                            t!("Updates come with the released app.")
                        }
                        .into_owned(),
                    ),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::{current, init_at, step_text};
    use crate::theme;
    use gpui_kit::TestAppContext;
    use gpui_kit::component::ActiveTheme as _;
    use playpen_core::{Appearance as Chosen, Settings};
    use typecase::theme::{Appearance, set_appearance, set_theme};

    fn scratch(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("playpen-{name}-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[gpui_kit::test]
    fn choosing_an_appearance_or_a_theme_saves_it_and_shows_it(cx: &mut TestAppContext) {
        let path = scratch("settings");
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            init_at(Some(path.clone()), cx);
            set_appearance(Appearance::Dark, cx);
            assert_eq!(current(cx).appearance, Chosen::Dark);
            assert!(cx.theme().mode.is_dark());
            set_theme(gpui_kit::component::ThemeMode::Dark, "Nightbrew Cove", cx);
            assert_eq!(current(cx).dark_theme, "Nightbrew Cove");
        });
        let saved = Settings::load(&path);
        assert_eq!(saved.appearance, Chosen::Dark);
        assert_eq!(saved.dark_theme, "Nightbrew Cove");
        cx.update(|cx| {
            set_appearance(Appearance::Light, cx);
            assert!(!cx.theme().mode.is_dark());
        });
        let _ = std::fs::remove_file(&path);
    }

    #[gpui_kit::test]
    fn text_size_steps_scale_the_theme_and_are_saved(cx: &mut TestAppContext) {
        let path = scratch("text");
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            init_at(Some(path.clone()), cx);
            let actual = cx.theme().font_size;
            step_text(1, cx);
            assert!(cx.theme().font_size > actual);
            // A change of theme keeps the size.
            set_appearance(Appearance::Dark, cx);
            assert!(cx.theme().font_size > actual);
            step_text(0, cx);
            assert_eq!(cx.theme().font_size, actual);
        });
        assert_eq!(Settings::load(&path).text_scale, 1.0);
        let _ = std::fs::remove_file(&path);
    }
}
