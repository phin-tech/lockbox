//! The window's content: the sandboxes on OpenShell's gateway, or New Agent…'s form in their place, under the title
//! bar, with the cards over the window (the palette and Keys).

mod activity;
mod agents;
mod bring_back;
mod gateways;
mod guide;
mod images;
mod kits;
mod making;
mod microvm;
mod ports;
mod requests;
mod sandboxes;
mod services;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use gpui_kit::component::Icon;
use gpui_kit::component::button::DropdownButton;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Sizable as _, StyledExt as _, TitleBar,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use playpen_core::{
    AddedFolders, Agent, AgentState, Gateway, GatewayInfo, OpenShell, OpenShellError, Place,
    Sandbox, What,
};
use rust_i18n::t;
use typecase::Toast;
use typecase::help::{HelpEvent, KeyHelp};
use typecase::overlay;
use typecase::palette::{CommandPalette, PaletteEvent};
use typecase::shell::{self, Footer, Lead};
use typecase::tour::TourTargetExt as _;

use crate::commands::{self, COMMANDS};
use crate::providers::{Providers, ProvidersEvent};
use crate::shortcuts::{CONTEXT, DismissForm, FORM_CONTEXT, RunAction};
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
        .unwrap_or(980.);
    let centered = WindowBounds::Windowed(Bounds::centered(None, size(px(width), px(660.)), cx));
    let restored = remembered && !cfg!(feature = "screenshot");
    let bounds = restored.then(|| restore::bounds(cx)).flatten();
    let options = WindowOptions {
        window_bounds: Some(bounds.unwrap_or(centered)),
        window_min_size: Some(size(px(680.), px(400.))),
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
    /// The gateways OpenShell has registered, as last listed.
    gateways: Vec<GatewayInfo>,
    /// The gateway whose sandboxes are shown, and which every call goes to. It's the active one until the gateways
    /// have been listed, and until another is chosen.
    gateway: Gateway,
    /// The sandboxes of each gateway as last listed, by the gateway's name (empty before they're listed), or why they
    /// couldn't be. A gateway that was never asked isn't in it.
    lists: BTreeMap<String, Listing>,
    /// The gateways whose sandboxes the sidebar shows under them, which are asked every few seconds; the others aren't
    /// asked at all, so several gateways don't mean several processes every few seconds.
    expanded: BTreeSet<String>,
    /// The chosen sandbox, which the Sandbox menu's Commands act on.
    selected: Option<SandboxKey>,
    /// Where Playpen notes the folders added to sandboxes since they were made; none in a capture or a test.
    added: Option<AddedFolders>,
    /// What's being done to a sandbox, while it's done.
    working: BTreeMap<SandboxKey, Task<()>>,
    /// Why herdr-core's sessions couldn't be read last time, so it's logged once rather than every few seconds.
    herdr_failed: Option<String>,
    /// What the agent in each sandbox's sessions is doing, by sandbox, when herdr-core keeps them.
    agent_states: BTreeMap<String, AgentState>,
    /// The last thing done that failed, or a note on it, until something else is done.
    failed: Option<String>,
    making: Option<making::Making>,
    /// Bringing the chosen sandbox's work back to the Mac.
    bring_back: bring_back::BringBack,
    /// What's attached to the chosen sandbox besides its model's key.
    services: services::Services,
    /// Each sandbox's network requests, waiting and decided.
    requests: requests::Requests,
    /// The chosen sandbox's recent network activity.
    activity: activity::ActivityLog,
    /// The ports forwarded to sandboxes.
    ports: ports::Ports,
    /// The place shown, chosen in the sidebar.
    page: Page,
    agents: agents::AgentsPage,
    kits: kits::KitsPage,
    /// Asking OpenShell for what kits can hold.
    kits_asking: Option<Task<()>>,
    /// The Providers place, made the first time it's shown, since it asks OpenShell for its providers.
    providers: Option<Entity<Providers>>,
    /// A gateway being added, in the detail panel's place.
    adding: Option<gateways::Adding>,
    /// Where the shown gateway gets its images, being chosen, in the detail panel's place.
    imaging: Option<images::Imaging>,
    /// The MicroVM gateway being set up, in the detail panel's place.
    microvm: Option<microvm::MicroVmForm>,
    /// What's being done to a gateway that takes a while, which the sidebar says.
    gateway_working: Option<gateways::GatewayWork>,
    card: Option<Card>,
    /// The welcome, the tour of the window or setting up, while one runs.
    guide: Option<guide::Guide>,
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
            // The first, Ready, has network requests waiting.
            let key = SandboxKey {
                gateway: view.gateway_key(),
                name: "claude-patchbay".to_string(),
            };
            view.requests
                .by_sandbox
                .insert(key.clone(), requests::stand_in_requests());
            view.ports_give_stand_ins(&key);
            view.give_stand_in_services(&key);
            view.selected = Some(key);
            view.give_stand_in_activity();
            return view;
        }
        let view = Self::watching(Watch::OpenShell, window, cx);
        // `PLAYPEN_SHOT_GUIDE=welcome|tour|setup` shows a capture the guide; otherwise a first launch welcomes.
        let stage = if cfg!(feature = "screenshot") {
            match std::env::var("PLAYPEN_SHOT_GUIDE").as_deref() {
                Ok("welcome") => Some(guide::Stage::Welcome),
                Ok("tour") => Some(guide::Stage::Tour),
                Ok("setup") => Some(guide::Stage::Setup),
                _ => None,
            }
        } else {
            (!crate::restore::welcomed(cx)).then_some(guide::Stage::Welcome)
        };
        if let Some(stage) = stage {
            cx.defer_in(window, move |this, window, cx| {
                this.start_guide(stage, window, cx)
            });
        }
        view
    }

    /// A view that lists OpenShell's sandboxes, or, with [`Watch::Nothing`], whatever it's given.
    pub fn watching(watch: Watch, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = theme::follow_system(window, cx);
        let bounds =
            cx.observe_window_bounds(window, |_, window, cx| restore::remember(window, cx));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let watching = (watch == Watch::OpenShell).then(|| Self::watch_sandboxes(window, cx));
        let ports = ports::Ports::new(watch, window, cx);
        let services = services::Services::new(window, cx);
        Self {
            focus,
            openshell: OpenShell::find(),
            watch,
            gateways: Vec::new(),
            gateway: Gateway::Active,
            lists: BTreeMap::new(),
            expanded: BTreeSet::new(),
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
            services,
            requests: requests::Requests::default(),
            activity: activity::ActivityLog::default(),
            ports,
            bring_back: bring_back::BringBack::default(),
            page: Page::Sandboxes,
            agents: agents::AgentsPage::new(watch),
            kits: kits::KitsPage::new(watch),
            kits_asking: None,
            providers: None,
            adding: None,
            imaging: None,
            microvm: None,
            gateway_working: None,
            card: None,
            guide: None,
            _watching: watching,
            _appearance: appearance,
            _bounds: bounds,
        }
    }

    /// Does what a menu item or a shortcut named: the Commands that are this view's own, then the rest.
    fn run(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        match name {
            "sandbox.new" => self.start_making(window, cx),
            "agent.new-custom" => self.edit_custom_agent(None, window, cx),
            "help.welcome" => self.start_guide(guide::Stage::Welcome, window, cx),
            "help.tour" => self.start_guide(guide::Stage::Tour, window, cx),
            "help.setup" => self.start_guide(guide::Stage::Setup, window, cx),
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
            "sandbox.bring-back" => self.bring_back_selected(window, cx),
            "sandbox.approve-requests" => self.approve_all_requests(window, cx),
            "sandbox.activity" => self.look_up_activity(cx),
            "sandbox.forward" => self.focus_port_field(window, cx),
            "sandbox.start" => self.act_on_selected(sandboxes::Act::Start, window, cx),
            "sandbox.restart" => self.act_on_selected(sandboxes::Act::Restart, window, cx),
            "sandbox.stop" => self.act_on_selected(sandboxes::Act::Stop, window, cx),
            "sandbox.delete" => self.confirm_delete(window, cx),
            "sandbox.next" => match self.page {
                Page::Agents => self.select_next_agent(1, window, cx),
                Page::Kits => self.select_next_kit(1, window, cx),
                Page::Sandboxes | Page::Providers => self.select_next(1, cx),
            },
            "sandbox.previous" => match self.page {
                Page::Agents => self.select_next_agent(-1, window, cx),
                Page::Kits => self.select_next_kit(-1, window, cx),
                Page::Sandboxes | Page::Providers => self.select_next(-1, cx),
            },
            "go.kits" => self.show_page(Page::Kits, window, cx),
            "go.sandboxes" => self.show_page(Page::Sandboxes, window, cx),
            "go.agents" => self.show_page(Page::Agents, window, cx),
            "go.providers" => self.show_page(Page::Providers, window, cx),
            "sandboxes.refresh" => self.refresh(window, cx),
            "gateway.next" => self.switch_gateway(1, window, cx),
            "gateway.previous" => self.switch_gateway(-1, window, cx),
            "gateway.add" => self.start_adding_gateway(window, cx),
            "gateway.microvm" => self.start_microvm_form(window, cx),
            "gateway.images" => self.start_images_form(window, cx),
            "gateway.remove" => {
                if let Some(name) = self.gateway.name().map(str::to_string) {
                    self.confirm_remove_gateway(name, window, cx);
                }
            }
            "gateway.sign-in" => {
                if let Some(name) = self.gateway.name().map(str::to_string) {
                    self.sign_in_to_gateway(name, window, cx);
                }
            }
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
                // `PLAYPEN_SHOT_ENV=<file>` shows a capture a kit being made from that `.env` file.
                if cfg!(feature = "screenshot")
                    && self.kits.import.is_none()
                    && let Some(path) = std::env::var_os("PLAYPEN_SHOT_ENV")
                {
                    self.read_env_file(path.into(), window, cx);
                    return;
                }
                if self.kits.selected.is_some()
                    && let Some(name) = self.kits.selected.clone()
                {
                    self.select_kit(name, window, cx);
                }
            }
            Page::Providers if self.providers.is_none() => {
                let providers = cx.new(|cx| Providers::new(window, cx));
                cx.subscribe_in(
                    &providers,
                    window,
                    |this, _, event: &ProvidersEvent, window, cx| match event {
                        ProvidersEvent::ServiceAdded(name) => {
                            if this.services.pending_attach.is_some() {
                                this.attach_newly_added_service(name.clone(), window, cx);
                            } else {
                                this.services.picker_dirty = true;
                            }
                        }
                        ProvidersEvent::Saved => {
                            this.guide_done(guide::GuideStep::SignIn, window, cx);
                        }
                        ProvidersEvent::ServiceCancelled => {
                            if this.services.pending_attach.take().is_some() {
                                this.show_page(Page::Sandboxes, window, cx);
                            }
                        }
                    },
                )
                .detach();
                self.providers = Some(providers);
            }
            Page::Sandboxes => {
                self.services.picker_dirty = true;
            }
            Page::Providers => {}
        }
        self.page = page;
        // The guide holds the keyboard while it shows a place, so Return and the arrows still go through its steps.
        if self.guide.is_none() {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    fn close_card(&mut self, cx: &mut Context<Self>) {
        self.card = None;
        cx.notify();
    }

    /// Dismisses whichever inline form is open in the detail panel, if it isn't mid-operation.
    fn dismiss_active_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page == Page::Agents && self.agents.form.is_some() {
            self.cancel_custom_form(window, cx);
        } else if self.microvm.is_some() {
            self.cancel_microvm(window, cx);
        } else if self.imaging.is_some() {
            self.cancel_images(window, cx);
        } else if self.adding.is_some() {
            self.cancel_adding(window, cx);
        } else if self.making.is_some() {
            if !matches!(
                self.making.as_ref().map(|making| &making.state),
                Some(making::MakeState::Working(_))
            ) {
                self.cancel_making(window, cx);
            }
        } else if self.kits.import.is_some() {
            self.cancel_import(window, cx);
        }
    }
}

/// How wide the sidebar of places is, which the traffic lights sit above.
const NAV_WIDTH: f32 = 148. / 13.;

/// How wide the list of sandboxes is.
const LIST_WIDTH: f32 = 244. / 13.;

/// The title bar's height: taller than the kit's, since the list's heading and its buttons sit in it.
const TITLE_BAR_HEIGHT: Pixels = px(46.);

/// A sandbox, by its gateway and its name: two gateways can each have one of a name. The gateway is empty before the
/// gateways are listed, which is when there's only the active one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct SandboxKey {
    gateway: String,
    name: String,
}

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
            .px_4()
            .h(rems(38. / 13.))
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

    /// Standard shell for inline forms in the detail panel: header with close `×`, scrollable body, and a sticky
    /// footer holding status/error on the left and `Cancel` + primary button on the right. Escape dismisses it.
    #[allow(clippy::too_many_arguments)]
    fn form_panel(
        &self,
        title: impl Into<SharedString>,
        scroll_id: &'static str,
        body: Div,
        footer_left: impl IntoElement,
        cancel: Button,
        primary: Button,
        close_disabled: bool,
        on_close: impl Fn(&mut Self, &ClickEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let header = Self::panel_header(title, cx).child(
            Button::new(ElementId::from((ElementId::from("close-form"), scroll_id)))
                .ghost()
                .xsmall()
                .disabled(close_disabled)
                .icon(assets::IconName::X)
                .tooltip(t!("Cancel").into_owned())
                .on_click(cx.listener(on_close)),
        );
        let footer = h_flex()
            .flex_none()
            .justify_between()
            .items_center()
            .gap_3()
            .px_4()
            .py_2p5()
            .border_t_1()
            .border_color(theme.border)
            .child(div().flex_1().min_w_0().child(footer_left))
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(cancel)
                    .child(div().child(primary).tour_target("form-primary")),
            );
        v_flex()
            .size_full()
            .key_context(FORM_CONTEXT)
            .on_action(cx.listener(|this, _: &DismissForm, window, cx| {
                this.dismiss_active_form(window, cx);
            }))
            .child(header)
            .child(div().id(scroll_id).flex_1().overflow_y_scroll().child(body))
            .child(footer)
            .into_any_element()
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

/// A selectable choice pill inside a form: builds on Typecase's chip so unselected choices still have a clear hairline
/// border and recessed fill instead of looking like unclickable text.
pub(crate) fn choice_chip(selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let chip = shell::chip(selected, cx);
    if selected {
        chip
    } else {
        chip.border_color(theme.border.opacity(0.75))
            .bg(shell::recessed_colour(theme))
    }
}

/// A shell command in a code chip paired with a one-click Copy button that confirms with a Toast.
pub(crate) fn copyable_command(id: &'static str, command: impl Into<String>, cx: &App) -> Div {
    let command = command.into();
    let copied = command.clone();
    h_flex()
        .gap_1()
        .items_center()
        .child(shell::code_chip(command, cx))
        .child(
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(assets::IconName::Copy)
                .tooltip(t!("Copy").into_owned())
                .on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                    Toast::new(t!("Copied command").into_owned())
                        .key("copy-command")
                        .show(window, cx);
                }),
        )
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

/// A line saying what something means, small and quiet, capped at a comfortable reading measure.
fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .max_w(rems(42.))
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A one-line field at the kit's small size. The kit's Input fills whatever height it's offered, so it's given one.
fn small_field(state: &Entity<InputState>) -> Input {
    Input::new(state).small().h(rems(24. / 13.))
}

impl MainView {
    /// How many pending network requests or blocked agents need the person's attention across all polled gateways.
    fn attention_count(&self) -> usize {
        let ready = self.ready_sandboxes();
        let requests: usize = ready
            .iter()
            .map(|key| self.requests.pending(key).len())
            .sum();
        if requests > 0 {
            return requests;
        }
        ready
            .iter()
            .filter(|key| self.agent_states.get(&self.label_of(key)) == Some(&AgentState::Blocked))
            .count()
    }

    /// The sidebar of places, each with how many things it holds and an amber attention indicator when sandboxes wait.
    fn places(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let attention = self.attention_count();
        let rows = Page::ALL.map(|page| {
            let shown = self.page == page;
            let count = match page {
                Page::Sandboxes => Some(self.listed_sandboxes().len()),
                Page::Agents => Some(self.agents.count()),
                Page::Kits => Some(self.kits.kits.kits.len()),
                Page::Providers => self
                    .providers
                    .as_ref()
                    .and_then(|providers| providers.read(cx).total_count()),
            };
            let waiting_here = page == Page::Sandboxes && attention > 0;
            shell::sidebar_row(shown, cx)
                .id(ElementId::from((ElementId::from("place"), page.command())))
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .child(
                            Icon::new(page.icon())
                                .small()
                                .when(shown, |icon| icon.text_color(theme.primary)),
                        )
                        .child(div().truncate().text_sm().child(page.name())),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_1p5()
                        .items_center()
                        .when(waiting_here, |trail| {
                            trail.child(
                                div()
                                    .flex_none()
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(theme.warning),
                            )
                        })
                        .when_some(count.filter(|count| *count > 0), |trail, count| {
                            trail.child(shell::count(count, cx))
                        }),
                )
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
            .tour_target("places")
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_attach_picker(window, cx);
        let (heading, list, detail) = match self.page {
            Page::Sandboxes => (
                self.list_heading(cx),
                Some(self.sandbox_list(cx)),
                match (&self.microvm, &self.imaging, &self.adding, &self.making) {
                    (Some(microvm), ..) => self.microvm_panel(microvm, cx),
                    (None, Some(imaging), _, _) => self.images_panel(imaging, cx),
                    (None, None, Some(adding), _) => self.adding_panel(adding, cx),
                    (None, None, None, Some(making)) => self.making_panel(making, cx),
                    (None, None, None, None) => self.sandbox_detail(cx),
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
                h_flex().flex_1().h_full().px_3().gap_2().child(
                    v_flex()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_semibold()
                                .child(t!("Providers").into_owned()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t!("Model keys and services OpenShell holds").into_owned()),
                        ),
                ),
                None,
                match &self.providers {
                    Some(providers) => providers.clone().into_any_element(),
                    None => div().into_any_element(),
                },
            ),
        };
        let has_list = list.is_some();
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
                    // Over the places, where the traffic lights are, then over the list (or the full panel when there's no list).
                    .child(div().flex_none().w(rems(NAV_WIDTH)))
                    .child(heading)
                    .when(has_list, |bar| bar.child(div().flex_1())),
            )
            .child(
                // The places and the list are on the window's own colour, and what's chosen on a panel beside them.
                shell::columns(Lead::Sidebar)
                    .child(self.places(cx))
                    .when_some(list, |columns, list| {
                        columns.child(
                            div()
                                .flex_none()
                                .h_full()
                                .w(rems(LIST_WIDTH))
                                .child(list)
                                .tour_target("list"),
                        )
                    })
                    .child(
                        shell::panel(false, cx)
                            .flex_1()
                            .child(detail)
                            .tour_target("detail"),
                    ),
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
            .when_some(self.guide_banner(cx), |this, banner| this.child(banner))
            // Over everything, the card too, while the guide runs.
            .when_some(self.guide.as_ref(), |this, guide| {
                this.child(guide.tour.clone())
            })
    }
}
