// Not a glob: the kit's `test` macro would shadow the `#[test]` it expands to.
use super::{Listing, MainView, Watch};
use crate::commands::COMMANDS;
use crate::shortcuts::SHORTCUTS;
use crate::{palette, shortcuts, theme};
use gpui_kit::base::Root;
use gpui_kit::{AppContext as _, Context, Entity, TestAppContext, VisualTestContext};
use playpen_core::{Agent, OpenShellError, Phase, Provider, Sandbox};

/// Opens a window under the Root the app runs in, whose own bindings are part of what keys do. It asks OpenShell for
/// nothing; each test gives it the sandboxes it needs.
fn open(cx: &mut TestAppContext) -> (Entity<MainView>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        cx.bind_keys(shortcuts::bindings());
    });
    let view = std::rc::Rc::new(std::cell::RefCell::new(None));
    let (_, cx) = cx.add_window_view(|window, cx: &mut Context<Root>| {
        let main = cx.new(|cx| MainView::watching(Watch::Nothing, window, cx));
        *view.borrow_mut() = Some(main.clone());
        Root::new(main, window, cx)
    });
    let view = view.borrow_mut().take().unwrap();
    (view, cx)
}

fn sandbox(name: &str, phase: Phase) -> Sandbox {
    Sandbox {
        name: name.to_string(),
        phase,
        created: "2026-11-20 03:08:15".to_string(),
        agent: Some("claude-code".to_string()),
        folder: Some("/Users/sam/src/Patchbay".to_string()),
        workdir: Some("/sandbox/Patchbay".to_string()),
    }
}

fn listed(view: &Entity<MainView>, sandboxes: Vec<Sandbox>, cx: &mut VisualTestContext) {
    view.update(cx, |view, cx| view.listed(Ok(sandboxes), cx));
    cx.run_until_parked();
}

fn selected(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Option<String> {
    view.read_with(cx, |view, _| view.selected.clone())
}

#[gpui_kit::test]
fn the_first_sandbox_listed_is_chosen_and_the_arrows_move_through_them(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    listed(
        &view,
        vec![
            sandbox("alpha", Phase::Ready),
            sandbox("beta", Phase::Stopped),
        ],
        cx,
    );
    assert_eq!(selected(&view, cx).as_deref(), Some("alpha"));
    cx.simulate_keystrokes("down down");
    assert_eq!(
        selected(&view, cx).as_deref(),
        Some("beta"),
        "the last stays chosen"
    );
    cx.simulate_keystrokes("up");
    assert_eq!(selected(&view, cx).as_deref(), Some("alpha"));
}

#[gpui_kit::test]
fn stopped_sandboxes_are_listed_after_the_running_ones_and_the_arrows_follow_that_order(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    listed(
        &view,
        vec![
            sandbox("alpha", Phase::Stopped),
            sandbox("beta", Phase::Ready),
            sandbox("gamma", Phase::Error),
        ],
        cx,
    );
    assert_eq!(selected(&view, cx).as_deref(), Some("beta"));
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&view, cx).as_deref(), Some("gamma"));
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&view, cx).as_deref(), Some("alpha"));
}

#[gpui_kit::test]
fn a_chosen_sandbox_stays_chosen_until_its_gone(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    listed(
        &view,
        vec![
            sandbox("alpha", Phase::Ready),
            sandbox("beta", Phase::Ready),
        ],
        cx,
    );
    cx.simulate_keystrokes("down");
    listed(
        &view,
        vec![
            sandbox("alpha", Phase::Ready),
            sandbox("beta", Phase::Stopped),
        ],
        cx,
    );
    assert_eq!(selected(&view, cx).as_deref(), Some("beta"));
    listed(&view, vec![sandbox("alpha", Phase::Ready)], cx);
    assert_eq!(selected(&view, cx).as_deref(), Some("alpha"));
}

#[gpui_kit::test]
fn openshell_not_answering_is_said_rather_than_an_empty_list(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, cx| {
        view.listed(Err(OpenShellError::NotInstalled), cx);
    });
    view.read_with(cx, |view, _| {
        assert!(
            matches!(&view.sandboxes, Listing::Failed(error) if error.contains("brew install"))
        );
    });
}

#[gpui_kit::test]
fn new_agent_names_the_sandbox_after_the_folder_and_chooses_a_claude_provider(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    cx.simulate_keystrokes("cmd-n");
    cx.run_until_parked();
    view.update_in(cx, |view, window, cx| {
        view.providers_listed(
            Ok(vec![
                Provider {
                    name: "openrouter".to_string(),
                    kind: "openrouter".to_string(),
                },
                Provider {
                    name: "claude-sub".to_string(),
                    kind: "claude-code-oauth".to_string(),
                },
            ]),
            cx,
        );
        view.folder_chosen("/Users/sam/src/My App".into(), window, cx);
    });
    view.read_with(cx, |view, cx| {
        let making = view.making.as_ref().expect("New Agent… is showing");
        assert_eq!(making.chosen_provider(), Some("claude-sub"));
        assert_eq!(making.name_typed(cx), "claude-my-app");
    });
}

#[gpui_kit::test]
fn choosing_codex_renames_the_sandbox_and_chooses_a_chatgpt_provider(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    cx.simulate_keystrokes("cmd-n");
    cx.run_until_parked();
    view.update_in(cx, |view, window, cx| {
        view.providers_listed(
            Ok(vec![
                Provider {
                    name: "claude-sub".to_string(),
                    kind: "claude-code-oauth".to_string(),
                },
                Provider {
                    name: "chatgpt".to_string(),
                    kind: "playpen-chatgpt".to_string(),
                },
            ]),
            cx,
        );
        view.folder_chosen("/Users/sam/src/Patchbay".into(), window, cx);
        view.choose_agent(Agent::Codex, window, cx);
    });
    view.read_with(cx, |view, cx| {
        let making = view.making.as_ref().expect("New Agent… is showing");
        assert_eq!(making.chosen_provider(), Some("chatgpt"));
        assert_eq!(making.name_typed(cx), "codex-patchbay");
    });
}

#[gpui_kit::test]
fn a_name_typed_isnt_replaced_by_choosing_a_folder(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    cx.simulate_keystrokes("cmd-n");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("mine");
    view.update_in(cx, |view, window, cx| {
        view.folder_chosen("/Users/sam/src/Patchbay".into(), window, cx);
    });
    view.read_with(cx, |view, cx| {
        assert_eq!(view.making.as_ref().unwrap().name_typed(cx), "mine");
    });
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
    let source = [include_str!("mod.rs"), include_str!("../commands.rs")].concat();
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
    cx.simulate_keystrokes("cmd-k n e w space a g e n t enter");
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(view.making.is_some(), "New Agent… is showing");
        assert!(view.card.is_none(), "the palette has gone");
    });
}

#[gpui_kit::test]
fn keys_opens_over_the_window_and_escape_closes_it(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    listed(
        &view,
        vec![
            sandbox("alpha", Phase::Ready),
            sandbox("beta", Phase::Ready),
        ],
        cx,
    );
    cx.simulate_keystrokes("cmd-/");
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(view.card.is_some()));
    // Typing narrows the sheet rather than reaching the view.
    cx.simulate_keystrokes("down escape");
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(view.card.is_none()));
    assert_eq!(selected(&view, cx).as_deref(), Some("alpha"));
}

#[gpui_kit::test]
fn the_arrows_move_through_the_list_again_once_new_agent_is_cancelled(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    listed(
        &view,
        vec![
            sandbox("alpha", Phase::Ready),
            sandbox("beta", Phase::Ready),
        ],
        cx,
    );
    cx.simulate_keystrokes("cmd-n");
    cx.run_until_parked();
    view.update_in(cx, |view, window, cx| view.cancel_making(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&view, cx).as_deref(), Some("beta"));
}
