//! The window's content: the sandboxes on OpenShell's gateway, or New Agent…'s form in their place, under the title
//! bar, with the cards over the window (the palette and Keys).

mod agents;
mod kits;
mod making;
mod sandboxes;
mod services;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use gpui_kit::component::Icon;
use gpui_kit::component::button::DropdownButton;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Sizable as _, StyledExt as _, TitleBar,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use playpen_core::{
    AddedFolders, Agent, AgentState, OpenShell, OpenShellError, Place, Sandbox, What,
};
use rust_i18n::t;
use typecase::help::{HelpEvent, KeyHelp};
use typecase::overlay;
use typecase::palette::{CommandPalette, PaletteEvent};
use typecase::shell::{self, Footer, Lead};

use crate::commands::{self, COMMANDS};
use crate::providers::Providers;
use crate::shortcuts::{CONTEXT, RunAction};
use crate::{about, palette, restore, theme};

/// A place in the sidebar, each a kind of thing Playpen keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Sandboxes,
    Agents,
    Kits,
    Providers,
}

impl Page {
    const ALL: [Page; 4] = [Page::Sandboxes, Page::Agents, Page::Kits, Page::Providers];

    fn name(self) -> String {
        match self {
            Page::Sandboxes => t!("Sandboxes").into_owned(),
            Page::Agents => t!("Agents").into_owned(),
            Page::Kits => t!("Kits").into_owned(),
            Page::Providers => t!("Providers").into_owned(),
        }
    }

    fn icon(self) -> assets::IconName {
        match self {
            Page::Sandboxes => assets::IconName::Box,
            Page::Agents => assets::IconName::Bot,
            Page::Kits => assets::IconName::Package,
            Page::Providers => assets::IconName::KeyRound,
        }
    }

    /// The Command that shows it.
    fn command(self) -> &'static str {
        match self {
            Page::Sandboxes => "go.sandboxes",
            Page::Agents => "go.agents",
            Page::Kits => "go.kits",
            Page::Providers => "go.providers",
        }
    }
}

/// A Typecase card over the window, which holds the keyboard until it's done.
enum Card {
    Palette(Entity<CommandPalette>),
    Keys(Entity<KeyHelp>),
}

/// Opens a window on the app; with `remembered`, where the last one was when the app quit.
pub fn open(remembered: bool, cx: &mut App) -> Option<AnyWindowHandle> {
    open_with(remembered, cx, MainView::new)
}

/// Opens a window on the view `build` makes.
pub fn open_with(
    remembered: bool,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut Context<MainView>) -> MainView,
) -> Option<AnyWindowHandle> {
    // `PLAYPEN_WIDTH` narrows a capture, for checking a layout at a width of its own.
    let width = std::env::var("PLAYPEN_WIDTH")
        .ok()
        .filter(|_| cfg!(feature = "screenshot"))
        .and_then(|width| width.parse().ok())
        .unwrap_or(900.);
    let centered = WindowBounds::Windowed(Bounds::centered(None, size(px(width), px(640.)), cx));
    let restored = remembered && !cfg!(feature = "screenshot");
    let bounds = restored.then(|| restore::bounds(cx)).flatten();
    let options = WindowOptions {
        window_bounds: Some(bounds.unwrap_or(centered)),
        window_min_size: Some(size(px(480.), px(320.))),
        titlebar: Some(TitlebarOptions {
            title: Some("Playpen".into()),
            // Centred down the taller title bar, which holds the list's heading.
            traffic_light_position: Some(point(px(16.), px(16.))),
            ..TitleBar::title_bar_options()
        }),
        // Captures run hidden, so they never take over the screen.
        show: !cfg!(feature = "screenshot"),
        focus: !cfg!(feature = "screenshot"),
        ..TitleBar::window_options()
    };
    match gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| build(window, cx))) {
        Ok((handle, _view)) => Some(handle),
        Err(error) => {
            tracing::error!("no window: {error}");
            None
        }
    }
}

/// Whether a view asks OpenShell for its sandboxes. Tests give a view its sandboxes instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watch {
    OpenShell,
    Nothing,
}

pub struct MainView {
    focus: FocusHandle,
    /// The `openshell` command, or why there isn't one, looked for again at each refresh until it's found.
    openshell: Result<OpenShell, OpenShellError>,
    watch: Watch,
    /// The sandboxes as last listed, or why they couldn't be.
    sandboxes: Listing,
    /// The chosen sandbox, by name, which the Sandbox menu's Commands act on.
    selected: Option<String>,
    /// Where Playpen notes the folders added to sandboxes since they were made; none in a capture or a test.
    added: Option<AddedFolders>,
    /// What's being done to a sandbox, by its name, while it's done.
    working: BTreeMap<String, Task<()>>,
    /// Why herdr-core's sessions couldn't be read last time, so it's logged once rather than every few seconds.
    herdr_failed: Option<String>,
    /// What the agent in each sandbox's sessions is doing, by sandbox, when herdr-core keeps them.
    agent_states: BTreeMap<String, AgentState>,
    /// The last thing done that failed, or a note on it, until something else is done.
    failed: Option<String>,
    making: Option<making::Making>,
    /// What's attached to the chosen sandbox besides its model's key.
    services: services::Services,
    /// The place shown, chosen in the sidebar.
    page: Page,
    agents: agents::AgentsPage,
    kits: kits::KitsPage,
    /// Asking OpenShell for what kits can hold.
    kits_asking: Option<Task<()>>,
    /// The Providers place, made the first time it's shown, since it asks OpenShell for its providers.
    providers: Option<Entity<Providers>>,
    card: Option<Card>,
    _watching: Option<Task<()>>,
    _appearance: Subscription,
    _bounds: Subscription,
}

impl MainView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // `PLAYPEN_SHOT_SANDBOXES=1` gives a capture stand-in sandboxes, so the list can be seen full without making
        // any.
        if cfg!(feature = "screenshot") && std::env::var_os("PLAYPEN_SHOT_SANDBOXES").is_some() {
            let mut view = Self::watching(Watch::Nothing, window, cx);
            view.listed(Ok(sandboxes::stand_ins()), cx);
            return view;
        }
        Self::watching(Watch::OpenShell, window, cx)
    }

    /// A view that lists OpenShell's sandboxes, or, with [`Watch::Nothing`], whatever it's given.
    pub fn watching(watch: Watch, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = theme::follow_system(window, cx);
        let bounds =
            cx.observe_window_bounds(window, |_, window, cx| restore::remember(window, cx));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let watching = (watch == Watch::OpenShell).then(|| Self::watch_sandboxes(window, cx));
        Self {
            focus,
            openshell: OpenShell::find(),
            watch,
            sandboxes: Listing::Loading,
            selected: None,
            added: (watch == Watch::OpenShell)
                .then(|| crate::settings::file("added-folders"))
                .flatten()
                .map(AddedFolders::new),
            working: BTreeMap::new(),
            agent_states: BTreeMap::new(),
            herdr_failed: None,
            failed: None,
            making: None,
            services: services::Services::default(),
            page: Page::Sandboxes,
            agents: agents::AgentsPage::new(),
            kits: kits::KitsPage::new(watch),
            kits_asking: None,
            providers: None,
            card: None,
            _watching: watching,
            _appearance: appearance,
            _bounds: bounds,
        }
    }

    /// Does what a menu item or a shortcut named: the Commands that are this view's own, then the rest.
    fn run(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        match name {
            "sandbox.new" => self.start_making(window, cx),
            "sandbox.open" => self.open_session(None, What::Agent, window, cx),
            "sandbox.open-herdr" => {
                self.open_session(Some(Place::HerdrApp), What::Agent, window, cx)
            }
            "sandbox.open-ghostty" => {
                self.open_session(Some(Place::Ghostty), What::Agent, window, cx)
            }
            "sandbox.open-terminal" => {
                self.open_session(Some(Place::Terminal), What::Agent, window, cx);
            }
            "sandbox.continue" => self.open_session(None, What::Continue, window, cx),
            "sandbox.shell" => self.open_session(None, What::Shell, window, cx),
            "sandbox.add-folders" => self.choose_folders_to_add(window, cx),
            "sandbox.start" => self.act_on_selected(sandboxes::Act::Start, window, cx),
            "sandbox.restart" => self.act_on_selected(sandboxes::Act::Restart, window, cx),
            "sandbox.stop" => self.act_on_selected(sandboxes::Act::Stop, window, cx),
            "sandbox.delete" => self.confirm_delete(window, cx),
            "sandbox.next" => match self.page {
                Page::Agents => self.select_next_agent(1, cx),
                Page::Kits => self.select_next_kit(1, window, cx),
                Page::Sandboxes | Page::Providers => self.select_next(1, cx),
            },
            "sandbox.previous" => match self.page {
                Page::Agents => self.select_next_agent(-1, cx),
                Page::Kits => self.select_next_kit(-1, window, cx),
                Page::Sandboxes | Page::Providers => self.select_next(-1, cx),
            },
            "go.kits" => self.show_page(Page::Kits, window, cx),
            "go.sandboxes" => self.show_page(Page::Sandboxes, window, cx),
            "go.agents" => self.show_page(Page::Agents, window, cx),
            "go.providers" => self.show_page(Page::Providers, window, cx),
            "sandboxes.refresh" => self.refresh(window, cx),
            "app.palette" => self.open_palette(window, cx),
            "app.keys" => self.open_keys(window, cx),
            _ => {
                let ran = commands::run(name, window, cx);
                debug_assert!(ran || !COMMANDS.contains(&name), "{name} has no case here");
            }
        }
    }

    /// Shows the palette over the window. What's chosen runs here, as if from its menu, once the palette has gone
    /// and given focus back.
    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let palette = cx.new(|cx| CommandPalette::new(&t!("run"), palette::commands(), window, cx));
        cx.subscribe_in(
            &palette,
            window,
            |this, _, event: &PaletteEvent, window, cx| match event {
                PaletteEvent::Run(name) => {
                    this.close_card(cx);
                    this.run(&name.clone(), window, cx);
                }
                PaletteEvent::Dismissed => this.close_card(cx),
                _ => {}
            },
        )
        .detach();
        self.card = Some(Card::Palette(palette));
        cx.notify();
    }

    /// Shows Keys over the window: every key, by its menu, narrowed by typing.
    fn open_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys = cx.new(|cx| KeyHelp::new(&t!("Keys"), about::key_sections(), window, cx));
        cx.subscribe_in(&keys, window, |this, _, _: &HelpEvent, _, cx| {
            this.close_card(cx)
        })
        .detach();
        self.card = Some(Card::Keys(keys));
        cx.notify();
    }

    /// Shows a place, asking for what it needs the first time.
    fn show_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        match page {
            Page::Agents => self.check_images(window, cx),
            Page::Kits => {
                self.look_up_catalog(cx);
                if self.kits.selected.is_some()
                    && let Some(name) = self.kits.selected.clone()
                {
                    self.select_kit(name, window, cx);
                }
            }
            Page::Providers if self.providers.is_none() => {
                self.providers = Some(cx.new(|cx| Providers::new(window, cx)));
            }
            Page::Sandboxes | Page::Providers => {}
        }
        self.page = page;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn close_card(&mut self, cx: &mut Context<Self>) {
        self.card = None;
        cx.notify();
    }
}

/// How wide the sidebar of places is, which the traffic lights sit above.
const NAV_WIDTH: f32 = 180. / 13.;

/// How wide the list of sandboxes is.
const LIST_WIDTH: f32 = 260. / 13.;

/// The title bar's height: taller than the kit's, since the list's heading and its buttons sit in it.
const TITLE_BAR_HEIGHT: Pixels = px(46.);

/// The sandboxes, as far as they're known.
enum Listing {
    Loading,
    Listed(Vec<Sandbox>),
    Failed(String),
}

impl MainView {
    /// A panel's heading: its title, and whatever goes beside it on the right.
    fn panel_header(title: impl Into<SharedString>, cx: &App) -> Div {
        h_flex()
            .flex_none()
            .justify_between()
            .gap_2()
            .px_3()
            .h(rems(36. / 13.))
            .border_b_1()
            .border_color(cx.theme().border)
            // The title gives way to what's beside it, so a button is never pushed out of a narrow panel.
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_semibold()
                    .child(title.into()),
            )
    }
}

/// A place's heading, in the title bar over its list: its name, a line about it, and whatever goes on the right.
fn heading(title: String, said: String, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .h_full()
        .w(rems(LIST_WIDTH))
        .px_3()
        .gap_2()
        .justify_between()
        .child(
            v_flex()
                .min_w_0()
                .child(div().truncate().text_sm().font_semibold().child(title))
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(said),
                ),
        )
}

/// A row of a list: the chosen one in the list's selection colour.
fn list_row(chosen: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let hover = theme.list_hover;
    h_flex()
        .flex_none()
        .gap_2p5()
        .px_2()
        .py_1p5()
        .rounded_lg()
        .cursor_pointer()
        .when(chosen, |row| row.bg(theme.list_active))
        .when(!chosen, |row| row.hover(move |row| row.bg(hover)))
}

/// Something in a detail panel: a small heading and what's under it.
fn section(title: impl Into<SharedString>, cx: &App) -> Div {
    v_flex().gap_2().child(
        div()
            .text_sm()
            .font_semibold()
            .text_color(cx.theme().muted_foreground)
            .child(title.into()),
    )
}

/// A line saying what something means, small and quiet.
fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A one-line field at the kit's small size. The kit's Input fills whatever height it's offered, so it's given one.
fn small_field(state: &Entity<InputState>) -> Input {
    Input::new(state).small().h(rems(24. / 13.))
}

impl MainView {
    /// The sidebar of places, each with how many things it holds.
    fn places(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let rows = Page::ALL.map(|page| {
            let shown = self.page == page;
            let count = match page {
                Page::Sandboxes => Some(self.listed_sandboxes().len()),
                Page::Agents => Some(Agent::ALL.len()),
                Page::Kits => Some(self.kits.kits.kits.len()),
                Page::Providers => None,
            };
            shell::sidebar_row(shown, cx)
                .id(ElementId::from((ElementId::from("place"), page.command())))
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .child(
                            Icon::new(page.icon())
                                .small()
                                .when(shown, |icon| icon.text_color(theme.accent_foreground)),
                        )
                        .child(div().truncate().text_sm().child(page.name())),
                )
                .when_some(count.filter(|count| *count > 0), |row, count| {
                    row.child(shell::count(count, cx))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run(page.command(), window, cx);
                }))
        });
        v_flex()
            .flex_none()
            .h_full()
            .w(rems(NAV_WIDTH))
            .px_2()
            .pt_1()
            .gap(px(2.))
            .border_r_1()
            .border_color(shell::hairline(theme))
            .children(rows)
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (heading, list, detail) = match self.page {
            Page::Sandboxes => (
                self.list_heading(cx),
                Some(self.sandbox_list(cx)),
                match &self.making {
                    Some(making) => self.making_panel(making, cx),
                    None => self.sandbox_detail(cx),
                },
            ),
            Page::Agents => (
                self.agents_heading(cx),
                Some(self.agent_list(cx)),
                self.agent_detail(cx),
            ),
            Page::Kits => (
                self.kits_heading(cx),
                Some(self.kit_list(cx)),
                self.kit_detail(cx),
            ),
            Page::Providers => (
                heading(
                    t!("Providers").into_owned(),
                    t!("Model keys and services OpenShell holds").into_owned(),
                    cx,
                ),
                None,
                match &self.providers {
                    Some(providers) => div()
                        .id("providers")
                        .size_full()
                        .overflow_y_scroll()
                        .child(providers.clone())
                        .into_any_element(),
                    None => div().into_any_element(),
                },
            ),
        };
        shell::window(cx)
            .relative()
            .track_focus(&self.focus)
            // The view's own keys only while it has focus itself: a binding here would otherwise take its letter
            // before a text field inside the view sees it.
            .key_context(if self.focus.is_focused(window) {
                CONTEXT
            } else {
                "Typing"
            })
            .on_action(cx.listener(|this, action: &RunAction, window, cx| {
                this.run(action.name, window, cx);
            }))
            .child(
                shell::title_bar(cx)
                    .h(TITLE_BAR_HEIGHT)
                    .pl_0()
                    // Over the places, where the traffic lights are, then over the list.
                    .child(div().flex_none().w(rems(NAV_WIDTH)))
                    .child(heading)
                    .child(div().flex_1()),
            )
            .child(
                // The places and the list are on the window's own colour, and what's chosen on a panel beside them.
                shell::columns(Lead::Sidebar)
                    .child(self.places(cx))
                    .when_some(list, |columns, list| {
                        columns.child(div().flex_none().h_full().w(rems(LIST_WIDTH)).child(list))
                    })
                    .child(shell::panel(false, cx).flex_1().child(detail)),
            )
            // Last, so it's over everything; a click outside it closes it.
            .when_some(self.card.as_ref(), |this, card| {
                let card = match card {
                    Card::Palette(palette) => palette.clone().into_any_element(),
                    Card::Keys(keys) => keys.clone().into_any_element(),
                };
                this.child(
                    overlay(card).on_dismiss(cx.listener(|this, _, _, cx| this.close_card(cx))),
                )
            })
    }
}
