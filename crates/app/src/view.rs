//! The window's content. Replace the Tally with the app's own; the title bar, Commands, focus and the cards over the
//! window (the palette and Keys) are the parts worth keeping.

use gpui_kit::component::{
    ActiveTheme, TitleBar,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lockbox_core::Tally;
use rust_i18n::t;
use typecase::help::{HelpEvent, KeyHelp};
use typecase::overlay;
use typecase::palette::{CommandPalette, PaletteEvent};

use crate::commands::{self, COMMANDS};
use crate::shortcuts::{CONTEXT, RunAction};
use crate::{about, palette, restore, theme};

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
    // `LOCKBOX_WIDTH` narrows a capture, for checking a layout at a width of its own.
    let width = std::env::var("LOCKBOX_WIDTH")
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
            title: Some("Lockbox".into()),
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

pub struct MainView {
    focus: FocusHandle,
    pub tally: Tally,
    card: Option<Card>,
    _appearance: Subscription,
    _bounds: Subscription,
}

impl MainView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = theme::follow_system(window, cx);
        let bounds =
            cx.observe_window_bounds(window, |_, window, cx| restore::remember(window, cx));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            focus,
            tally: Tally::default(),
            card: None,
            _appearance: appearance,
            _bounds: bounds,
        }
    }

    /// Does what a menu item or a shortcut named: the Commands that are this view's own, then the rest.
    fn run(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        match name {
            "tally.add" => {
                self.tally.add();
                cx.notify();
            }
            "tally.clear" => {
                self.tally.clear();
                cx.notify();
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

    fn close_card(&mut self, cx: &mut Context<Self>) {
        self.card = None;
        cx.notify();
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .relative()
            .size_full()
            .track_focus(&self.focus)
            // The view's own keys only while it has focus itself: a binding here would otherwise take its letter
            // before a text field inside the view sees it, so a field added later couldn't be typed in.
            .key_context(if self.focus.is_focused(window) {
                CONTEXT
            } else {
                "Typing"
            })
            .on_action(cx.listener(|this, action: &RunAction, window, cx| {
                this.run(action.name, window, cx);
            }))
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(TitleBar::new().child("Lockbox"))
            .child(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .child(div().text_3xl().child(self.tally.count().to_string()))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div().debug_selector(|| "add".into()).child(
                                    Button::new("add")
                                        .primary()
                                        .label(t!("Add one").into_owned())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.run("tally.add", window, cx);
                                        })),
                                ),
                            )
                            .child(
                                Button::new("clear")
                                    .outline()
                                    .label(t!("Clear").into_owned())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.run("tally.clear", window, cx);
                                    })),
                            ),
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
    }
}

#[cfg(test)]
mod tests {
    // Not a glob: the kit's `test` macro would shadow the `#[test]` it expands to.
    use super::MainView;
    use crate::commands::COMMANDS;
    use crate::shortcuts::SHORTCUTS;
    use crate::{palette, shortcuts, theme};
    use gpui_kit::base::Root;
    use gpui_kit::{
        AppContext as _, Context, Entity, Modifiers, TestAppContext, VisualTestContext,
    };

    /// Opens a window under the Root the app runs in, whose own bindings are part of what keys do.
    fn open(cx: &mut TestAppContext) -> (Entity<MainView>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            cx.bind_keys(shortcuts::bindings());
        });
        let view = std::rc::Rc::new(std::cell::RefCell::new(None));
        let (_, cx) = cx.add_window_view(|window, cx: &mut Context<Root>| {
            let main = cx.new(|cx| MainView::new(window, cx));
            *view.borrow_mut() = Some(main.clone());
            Root::new(main, window, cx)
        });
        let view = view.borrow_mut().take().unwrap();
        (view, cx)
    }

    #[gpui_kit::test]
    fn the_keyboard_adds_to_the_tally_and_clears_it(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_keystrokes("space space");
        view.read_with(cx, |view, _| assert_eq!(view.tally.count(), 2));
        cx.simulate_keystrokes("cmd-backspace");
        view.read_with(cx, |view, _| assert_eq!(view.tally.count(), 0));
    }

    #[gpui_kit::test]
    fn clicking_add_one_adds_one(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.run_until_parked();
        let button = cx.debug_bounds("add").expect("the Add one button is drawn");
        cx.simulate_click(button.center(), Modifiers::default());
        view.read_with(cx, |view, _| assert_eq!(view.tally.count(), 1));
    }

    #[gpui_kit::test]
    fn every_menu_item_and_shortcut_names_a_known_command(_: &mut TestAppContext) {
        for (_, name, _) in SHORTCUTS {
            assert!(COMMANDS.contains(name), "{name}");
        }
        for entry in palette::entries() {
            assert!(
                COMMANDS.contains(&entry.name),
                "{}: {}",
                entry.label,
                entry.name
            );
        }
    }

    #[gpui_kit::test]
    fn every_command_runs_and_can_be_reached(_: &mut TestAppContext) {
        // A debug_assert in `run` only catches a missing case when it's tried; this catches it in `cargo test`.
        let source = [include_str!("view.rs"), include_str!("commands.rs")].concat();
        let in_menus: Vec<_> = palette::entries()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        for name in COMMANDS {
            assert!(
                source.contains(&format!("\"{name}\" =>")),
                "{name} has no case in run"
            );
            let keyed = SHORTCUTS.iter().any(|(_, bound, _)| bound == name);
            assert!(
                in_menus.contains(name) || keyed,
                "{name} is in no menu and has no key"
            );
        }
    }

    #[gpui_kit::test]
    fn the_palette_runs_a_command_in_the_main_view(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_keystrokes("cmd-k a d d enter");
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(view.tally.count(), 1);
            assert!(view.card.is_none(), "the palette has gone");
        });
        // And the view's own keys work again, with focus back.
        cx.simulate_keystrokes("space");
        view.read_with(cx, |view, _| assert_eq!(view.tally.count(), 2));
    }

    #[gpui_kit::test]
    fn keys_opens_over_the_window_and_escape_closes_it(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_keystrokes("cmd-/");
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert!(view.card.is_some()));
        // Typing narrows the sheet rather than reaching the view.
        cx.simulate_keystrokes("space escape");
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.card.is_none());
            assert_eq!(view.tally.count(), 0);
        });
    }
}
