// Not a glob: the kit's `test` macro would shadow the `#[test]` it expands to.
use super::sandboxes::gateway_where;
use super::{Listing, MainView, Page, SandboxKey, Watch, agents, guide, microvm};
use crate::commands::COMMANDS;
use crate::providers::ShownGateway;
use crate::shortcuts::SHORTCUTS;
use crate::{palette, shortcuts, theme};
use gpui_kit::base::Root;
use gpui_kit::{AppContext as _, Context, Entity, TestAppContext, VisualTestContext};
use playpen_core::{
    Agent, DockerError, Gateway, GatewayInfo, GatewaySettings, ImageSourceError, ImagesFrom,
    OpenShell, OpenShellError, Phase, Platform, Provider, Sandbox,
};

/// Opens a window under the Root the app runs in, whose own bindings are part of what keys do. It asks OpenShell for
/// nothing; each test gives it the sandboxes it needs.
fn open(cx: &mut TestAppContext) -> (Entity<MainView>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        crate::settings::init_in_memory(cx);
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
        folders: vec!["/Users/sam/src/Patchbay".to_string()],
        workdir: Some("/sandbox/Patchbay".to_string()),
        custom: None,
    }
}

fn listed(view: &Entity<MainView>, sandboxes: Vec<Sandbox>, cx: &mut VisualTestContext) {
    view.update(cx, |view, cx| view.listed(Ok(sandboxes), cx));
    cx.run_until_parked();
}

fn selected(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Option<String> {
    view.read_with(cx, |view, _| {
        view.selected.as_ref().map(|key| key.name.clone())
    })
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
        assert!(matches!(view.listing(), Listing::Failed(error) if error.contains("brew install")));
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
        view.folders_added(vec!["/Users/sam/src/My App".into()], window, cx);
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
        view.folders_added(vec!["/Users/sam/src/Patchbay".into()], window, cx);
        view.choose_agent(Agent::Codex.into(), window, cx);
    });
    view.read_with(cx, |view, cx| {
        let making = view.making.as_ref().expect("New Agent… is showing");
        assert_eq!(making.chosen_provider(), Some("chatgpt"));
        assert_eq!(making.name_typed(cx), "codex-patchbay");
    });
}

#[gpui_kit::test]
fn choosing_pi_offers_only_api_keys_and_carries_the_model_typed_to_the_sandbox(
    cx: &mut TestAppContext,
) {
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
                    name: "openrouter".to_string(),
                    kind: "playpen-openrouter".to_string(),
                },
            ]),
            cx,
        );
        view.choose_agent(Agent::Pi.into(), window, cx);
        view.type_model("anthropic/claude-sonnet-4.5", window, cx);
    });
    view.read_with(cx, |view, cx| {
        let making = view.making.as_ref().expect("New Agent… is showing");
        assert_eq!(making.chosen_provider(), Some("openrouter"));
        assert_eq!(making.name_typed(cx), "pi");
        let model = making.model_for_plan(cx).expect("Pi is told its key");
        assert_eq!(model.kind, playpen_core::Kind::OpenRouter);
        assert_eq!(model.id.as_deref(), Some("anthropic/claude-sonnet-4.5"));
        assert_eq!(model.endpoints[0].host, "openrouter.ai");
    });
}

#[gpui_kit::test]
fn kits_fill_new_agent_with_all_their_services_the_later_ones_variables_and_the_first_ones_model_key(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    view.update(cx, |view, _| {
        view.kits.kits.kits.push(playpen_core::Kit {
            name: "work".to_string(),
            setup_folder: None,
            model_key: Some("claude-work".to_string()),
            services: vec!["github-work".to_string()],
            variables: vec![playpen_core::Variable {
                name: "NODE_ENV".to_string(),
                value: playpen_core::VariableValue::Value("production".to_string()),
            }],
        });
        view.kits.kits.kits.push(playpen_core::Kit {
            name: "local".to_string(),
            setup_folder: None,
            model_key: Some("claude-sub".to_string()),
            services: vec!["sentry".to_string()],
            variables: vec![playpen_core::Variable {
                name: "NODE_ENV".to_string(),
                value: playpen_core::VariableValue::Value("development".to_string()),
            }],
        });
    });
    cx.simulate_keystrokes("cmd-n");
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        view.use_kit("work", cx);
        view.use_kit("local", cx);
        view.providers_listed(
            Ok(vec![
                Provider {
                    name: "claude-sub".to_string(),
                    kind: "claude-code-oauth".to_string(),
                },
                Provider {
                    name: "claude-work".to_string(),
                    kind: "claude-code-oauth".to_string(),
                },
            ]),
            cx,
        );
    });
    view.read_with(cx, |view, _| {
        let making = view.making.as_ref().expect("New Agent… is showing");
        assert_eq!(
            making.chosen_provider(),
            Some("claude-work"),
            "the kit's, not the first"
        );
        assert_eq!(
            making.ticked_services(),
            ["github-work", "sentry"],
            "every kit's"
        );
        assert_eq!(
            making.variable_values(),
            ["NODE_ENV=development"],
            "the later kit's"
        );
    });
}

#[gpui_kit::test]
fn a_env_file_offers_its_known_keys_ticked_and_asks_where_the_rest_go(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let folder = std::env::temp_dir()
        .join(format!("pp-env-{}", std::process::id()))
        .join("Shop");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join(".env");
    std::fs::write(
        &file,
        "ANTHROPIC_API_KEY=sk-ant-1\nGITHUB_TOKEN=op://Work/GitHub/token\nLINEAR_API_KEY=lin_1\n",
    )
    .unwrap();
    view.update_in(cx, |view, window, cx| {
        view.read_env_file(file.clone(), window, cx)
    });
    view.read_with(cx, |view, cx| {
        assert_eq!(view.page, Page::Kits);
        let (kit, rows) = view.import_rows(cx);
        assert_eq!(kit, "shop");
        assert_eq!(
            rows,
            [
                (
                    "ANTHROPIC_API_KEY".to_string(),
                    true,
                    "anthropic".to_string(),
                    String::new()
                ),
                (
                    "GITHUB_TOKEN".to_string(),
                    true,
                    "github".to_string(),
                    "api.github.com".to_string()
                ),
                (
                    "LINEAR_API_KEY".to_string(),
                    false,
                    "linear".to_string(),
                    String::new()
                ),
            ]
        );
    });
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}

#[gpui_kit::test]
fn a_name_typed_isnt_replaced_by_choosing_a_folder(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    cx.simulate_keystrokes("cmd-n");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("mine");
    view.update_in(cx, |view, window, cx| {
        view.folders_added(vec!["/Users/sam/src/Patchbay".into()], window, cx);
    });
    view.read_with(cx, |view, cx| {
        assert_eq!(view.making.as_ref().unwrap().name_typed(cx), "mine");
    });
}

#[gpui_kit::test]
fn several_folders_name_the_sandbox_together_and_leaving_one_out_renames_it(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    cx.simulate_keystrokes("cmd-n");
    cx.run_until_parked();
    view.update_in(cx, |view, window, cx| {
        view.folders_added(
            vec![
                "/Users/sam/src/Patchbay".into(),
                "/Users/sam/src/Typecase".into(),
            ],
            window,
            cx,
        );
        // Adding one that's there already changes nothing.
        view.folders_added(vec!["/Users/sam/src/Patchbay".into()], window, cx);
    });
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.making.as_ref().unwrap().name_typed(cx),
            "claude-patchbay",
            "as many whole names as fit OpenShell's"
        );
    });
    view.update_in(cx, |view, window, cx| {
        view.remove_folder(std::path::Path::new("/Users/sam/src/Patchbay"), window, cx);
    });
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.making.as_ref().unwrap().name_typed(cx),
            "claude-typecase"
        );
    });
}

#[gpui_kit::test]
fn new_agent_from_agents_chooses_the_agent_shown_there_and_goes_back_to_sandboxes(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    cx.simulate_keystrokes("cmd-2 down");
    view.read_with(cx, |view, _| {
        assert_eq!(view.page, Page::Agents);
        assert_eq!(view.agents.selected, agents::Chosen::Agent(Agent::Codex));
    });
    cx.simulate_keystrokes("cmd-n");
    cx.run_until_parked();
    view.read_with(cx, |view, cx| {
        assert_eq!(view.page, Page::Sandboxes);
        assert_eq!(view.making.as_ref().unwrap().name_typed(cx), "codex");
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

/// Three gateways as OpenShell lists them: the one on this Mac, active, and two registered after it. Made up for these
/// tests, since only the local gateway's shape has been seen.
fn gateways() -> Vec<GatewayInfo> {
    let gateway = |name: &str, active: bool, remote: bool| GatewayInfo {
        name: name.to_string(),
        endpoint: format!("https://{name}.example.com"),
        active,
        kind: if remote { "remote" } else { "local" }.to_string(),
        is_remote: remote,
        auth: "mtls".to_string(),
    };
    // As `openshell gateway list` gives them: the active one first, then by name.
    vec![
        gateway("openshell", true, false),
        gateway("ec2", false, true),
        gateway("k8s-home", false, true),
    ]
}

fn shown(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Option<String> {
    view.read_with(cx, |view, _| view.gateway.name().map(str::to_string))
}

#[gpui_kit::test]
fn the_active_gateway_is_shown_first_and_the_keys_go_round_the_others(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, _| {
        view.openshell = Ok(OpenShell::new("/opt/homebrew/bin/openshell"));
        view.gateways_listed(Ok(gateways()));
    });
    assert_eq!(shown(&view, cx).as_deref(), Some("openshell"));
    listed(&view, vec![sandbox("alpha", Phase::Ready)], cx);
    cx.simulate_keystrokes("cmd-]");
    assert_eq!(shown(&view, cx).as_deref(), Some("ec2"));
    view.read_with(cx, |view, _| {
        assert!(
            matches!(view.listing(), Listing::Loading),
            "the last gateway's sandboxes aren't shown under this one"
        );
        assert_eq!(view.selected, None);
    });
    cx.simulate_keystrokes("cmd-] cmd-]");
    assert_eq!(shown(&view, cx).as_deref(), Some("openshell"), "it wraps");
    cx.simulate_keystrokes("cmd-[");
    assert_eq!(shown(&view, cx).as_deref(), Some("k8s-home"));
}

#[gpui_kit::test]
fn calls_go_to_the_gateway_that_is_shown(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, _| {
        view.openshell = Ok(OpenShell::new("/opt/homebrew/bin/openshell"));
        view.gateways_listed(Ok(gateways()));
    });
    cx.simulate_keystrokes("cmd-[");
    view.update(cx, |view, _| {
        let openshell = view.openshell().unwrap();
        assert_eq!(openshell.gateway(), &Gateway::named("k8s-home"));
        assert!(!view.current_gateway().is_local());
    });
}

#[gpui_kit::test]
fn with_one_gateway_there_is_nothing_to_go_to(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, _| {
        view.gateways_listed(Ok(gateways().into_iter().filter(|g| g.active).collect()));
    });
    cx.simulate_keystrokes("cmd-] cmd-[");
    assert_eq!(shown(&view, cx).as_deref(), Some("openshell"));
}

#[gpui_kit::test]
fn a_gateway_that_goes_is_replaced_by_the_active_one_and_a_failed_listing_changes_nothing(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    view.update(cx, |view, _| view.gateways_listed(Ok(gateways())));
    cx.simulate_keystrokes("cmd-]");
    assert_eq!(shown(&view, cx).as_deref(), Some("ec2"));
    view.update(cx, |view, _| {
        view.gateways_listed(Err(OpenShellError::NotInstalled));
    });
    assert_eq!(shown(&view, cx).as_deref(), Some("ec2"));
    view.update(cx, |view, _| {
        view.gateways_listed(Ok(gateways()
            .into_iter()
            .filter(|g| g.name != "ec2")
            .collect()));
    });
    assert_eq!(shown(&view, cx).as_deref(), Some("openshell"));
}

#[gpui_kit::test]
fn the_gateway_shown_is_the_one_settings_lists_providers_on(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let provided = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            cx.try_global::<ShownGateway>()
                .and_then(|shown| shown.0.name().map(str::to_string))
        })
    };
    view.update_in(cx, |view, window, cx| {
        view.gateways_listed(Ok(gateways()));
        view.show_gateway("k8s-home", window, cx);
    });
    assert_eq!(provided(cx).as_deref(), Some("k8s-home"));
    cx.simulate_keystrokes("cmd-]");
    assert_eq!(provided(cx).as_deref(), Some("openshell"), "it wraps round");
}

fn saved(
    view: &Entity<MainView>,
    name: &str,
    cx: &mut VisualTestContext,
) -> Option<GatewaySettings> {
    view.read_with(cx, |_, cx| {
        crate::settings::current(cx).gateways.get(name).cloned()
    })
}

fn key(gateway: &str, name: &str) -> SandboxKey {
    SandboxKey {
        gateway: gateway.to_string(),
        name: name.to_string(),
    }
}

/// Two gateways open, each with the sandboxes given.
fn two_open(
    view: &Entity<MainView>,
    here: Vec<Sandbox>,
    there: Vec<Sandbox>,
    cx: &mut VisualTestContext,
) {
    view.update_in(cx, |view, window, cx| {
        view.gateways_listed(Ok(gateways()));
        view.listed_for("openshell", Ok(here), cx);
        view.show_gateway("k8s-home", window, cx);
        view.listed_for("k8s-home", Ok(there), cx);
    });
}

#[gpui_kit::test]
fn two_gateways_can_each_have_a_sandbox_of_one_name_and_each_is_chosen_on_its_own(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    two_open(
        &view,
        vec![sandbox("claude", Phase::Ready)],
        vec![sandbox("claude", Phase::Stopped)],
        cx,
    );
    view.update(cx, |view, cx| view.select(key("openshell", "claude"), cx));
    view.read_with(cx, |view, _| {
        assert_eq!(view.selected_sandbox().unwrap().phase, Phase::Ready);
        assert_eq!(
            view.gateway.name(),
            Some("openshell"),
            "choosing it brings its gateway into focus"
        );
    });
    view.update(cx, |view, cx| view.select(key("k8s-home", "claude"), cx));
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.selected_sandbox().unwrap().phase,
            Phase::Stopped,
            "the other one, not a sandbox by that name"
        );
        assert_eq!(view.gateway.name(), Some("k8s-home"));
    });
}

#[gpui_kit::test]
fn the_arrows_go_down_each_open_gateways_sandboxes_in_the_order_the_sidebar_shows_them(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    two_open(
        &view,
        vec![sandbox("a", Phase::Ready), sandbox("b", Phase::Stopped)],
        vec![sandbox("c", Phase::Ready)],
        cx,
    );
    view.read_with(cx, |view, _| {
        let order: Vec<_> = view
            .visible_sandboxes()
            .into_iter()
            .map(|key| format!("{}/{}", key.gateway, key.name))
            .collect();
        assert_eq!(order, ["openshell/a", "openshell/b", "k8s-home/c"]);
    });
    view.update(cx, |view, cx| view.select(key("openshell", "b"), cx));
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&view, cx).as_deref(), Some("c"));
    view.read_with(cx, |view, _| {
        assert_eq!(view.gateway.name(), Some("k8s-home"))
    });
    cx.simulate_keystrokes("down");
    assert_eq!(
        selected(&view, cx).as_deref(),
        Some("c"),
        "the last stays chosen"
    );
    cx.simulate_keystrokes("up up");
    assert_eq!(selected(&view, cx).as_deref(), Some("a"));
}

#[gpui_kit::test]
fn closing_the_gateway_in_focus_hides_its_sandboxes_from_the_arrows_but_it_is_still_asked(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    two_open(
        &view,
        vec![sandbox("a", Phase::Ready)],
        vec![sandbox("c", Phase::Ready)],
        cx,
    );
    view.update_in(cx, |view, window, cx| {
        view.click_gateway("k8s-home", window, cx)
    });
    view.read_with(cx, |view, _| {
        assert_eq!(view.visible_sandboxes(), [key("openshell", "a")]);
        assert!(
            view.polled().contains(&"k8s-home".to_string()),
            "the one in focus is asked whatever it shows"
        );
    });
    view.update_in(cx, |view, window, cx| {
        view.click_gateway("k8s-home", window, cx)
    });
    view.read_with(cx, |view, _| assert_eq!(view.visible_sandboxes().len(), 2));
}

#[gpui_kit::test]
fn only_the_open_gateways_and_the_one_in_focus_are_asked(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, _| view.gateways_listed(Ok(gateways())));
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.polled(),
            ["openshell"],
            "ec2 and k8s-home were never opened"
        );
        assert!(view.lists.is_empty());
    });
    view.update_in(cx, |view, window, cx| view.show_gateway("ec2", window, cx));
    view.read_with(cx, |view, _| {
        assert_eq!(view.polled(), ["ec2", "openshell"])
    });
}

#[gpui_kit::test]
fn a_sandbox_is_known_to_herdr_by_its_name_on_this_macs_gateway_and_by_its_gateway_too_elsewhere(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    view.update(cx, |view, _| view.gateways_listed(Ok(gateways())));
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.label_of(&key("openshell", "claude")),
            "claude",
            "so sessions kept before there were gateways are found"
        );
        assert_eq!(view.label_of(&key("k8s-home", "claude")), "k8s-home/claude");
        assert_eq!(view.label_of(&key("", "claude")), "claude");
    });
}

#[gpui_kit::test]
fn what_was_kept_for_a_gateway_that_goes_goes_with_it(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    two_open(
        &view,
        vec![sandbox("a", Phase::Ready)],
        vec![sandbox("c", Phase::Ready)],
        cx,
    );
    view.update(cx, |view, cx| view.select(key("k8s-home", "c"), cx));
    view.update(cx, |view, _| {
        view.gateways_listed(Ok(gateways()
            .into_iter()
            .filter(|g| g.name != "k8s-home")
            .collect()));
    });
    view.read_with(cx, |view, _| {
        assert!(!view.lists.contains_key("k8s-home"));
        assert!(!view.expanded.contains("k8s-home"));
        assert_eq!(
            view.selected, None,
            "a sandbox chosen there isn't chosen any more"
        );
        assert_eq!(view.gateway.name(), Some("openshell"));
    });
}

#[gpui_kit::test]
fn a_gateway_that_fails_to_answer_keeps_the_others_listings(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    two_open(
        &view,
        vec![sandbox("a", Phase::Ready)],
        vec![sandbox("c", Phase::Ready)],
        cx,
    );
    view.update(cx, |view, cx| {
        view.listed_for(
            "k8s-home",
            Err(OpenShellError::Run("connection refused".to_string())),
            cx,
        );
    });
    view.read_with(cx, |view, _| {
        assert!(matches!(view.listing_of("k8s-home"), Listing::Failed(_)));
        assert!(matches!(view.listing_of("openshell"), Listing::Listed(_)));
        assert_eq!(view.visible_sandboxes(), [key("openshell", "a")]);
    });
}

#[gpui_kit::test]
fn a_gateway_not_on_this_mac_needs_a_registry_and_a_platform_before_its_image_source_is_saved(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    view.update_in(cx, |view, window, cx| {
        view.gateways_listed(Ok(gateways()));
        view.show_gateway("k8s-home", window, cx);
        view.run("gateway.images", window, cx);
    });
    view.read_with(cx, |view, _| {
        let imaging = view.imaging.as_ref().unwrap();
        assert_eq!(
            imaging.from(),
            ImagesFrom::Registry,
            "a cluster can't share this Mac's Docker"
        );
    });
    // Nothing yet: it says what's missing, and saves nothing.
    view.update_in(cx, |view, window, cx| view.save_images(window, cx));
    view.read_with(cx, |view, _| {
        let said = view.imaging.as_ref().unwrap().failure().unwrap();
        assert!(said.contains("needs one"), "{said}");
    });
    assert_eq!(saved(&view, "k8s-home", cx), None);
    // A registry, but no platform.
    view.update_in(cx, |view, window, cx| {
        view.imaging
            .as_ref()
            .unwrap()
            .type_registry("ghcr.io/phin-tech", window, cx);
        view.save_images(window, cx);
    });
    view.read_with(cx, |view, _| {
        let said = view.imaging.as_ref().unwrap().failure().unwrap();
        assert!(said.contains("platform"), "{said}");
    });
    assert_eq!(saved(&view, "k8s-home", cx), None);
    // Both.
    view.update_in(cx, |view, window, cx| {
        view.choose_images(ImagesFrom::Registry, Some(Platform::Amd64), cx);
        view.save_images(window, cx);
    });
    view.read_with(cx, |view, _| assert!(view.imaging.is_none()));
    assert_eq!(
        saved(&view, "k8s-home", cx),
        Some(GatewaySettings {
            images: Some(ImagesFrom::Registry),
            registry: "ghcr.io/phin-tech".to_string(),
            platform: Some(Platform::Amd64),
        })
    );
    // And a sandbox can now be made from it.
    let info = view.read_with(cx, |view, _| view.current_gateway());
    let settings = view.read_with(cx, |_, cx| crate::settings::current(cx));
    // The only thing left in the way is Docker itself, which this test says isn't installed.
    let left = settings
        .image_source(&info, || Err(DockerError::NotInstalled))
        .err();
    assert!(matches!(
        left,
        Some(ImageSourceError::Docker(DockerError::NotInstalled))
    ));
}

#[gpui_kit::test]
fn the_gateway_on_this_mac_starts_on_its_docker_and_keeps_no_registry_with_it(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    view.update_in(cx, |view, window, cx| {
        view.gateways_listed(Ok(gateways()));
        view.run("gateway.images", window, cx);
    });
    view.read_with(cx, |view, _| {
        assert_eq!(view.imaging.as_ref().unwrap().from(), ImagesFrom::Docker);
    });
    view.update_in(cx, |view, window, cx| {
        view.imaging
            .as_ref()
            .unwrap()
            .type_registry("ghcr.io/left-over", window, cx);
        view.save_images(window, cx);
    });
    assert_eq!(
        saved(&view, "openshell", cx),
        Some(GatewaySettings {
            images: Some(ImagesFrom::Docker),
            registry: String::new(),
            platform: None,
        })
    );
}

#[gpui_kit::test]
fn showing_another_gateway_closes_the_image_source_form(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update_in(cx, |view, window, cx| {
        view.gateways_listed(Ok(gateways()));
        view.run("gateway.images", window, cx);
        view.show_gateway("ec2", window, cx);
    });
    view.read_with(cx, |view, _| assert!(view.imaging.is_none()));
    assert_eq!(saved(&view, "openshell", cx), None, "cancelled, not saved");
}

#[gpui_kit::test]
fn add_gateway_shows_its_form_in_place_of_the_sandbox_and_cancel_closes_it(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    view.update(cx, |view, _| view.gateways_listed(Ok(gateways())));
    listed(&view, vec![sandbox("alpha", Phase::Ready)], cx);
    view.update_in(cx, |view, window, cx| {
        view.run("gateway.add", window, cx);
    });
    view.read_with(cx, |view, _| {
        assert!(view.adding.is_some() && view.making.is_none());
    });
    // New Agent… takes its place, and the other way round.
    cx.simulate_keystrokes("cmd-n");
    view.read_with(cx, |view, _| {
        assert!(view.adding.is_none() && view.making.is_some());
    });
    view.update_in(cx, |view, window, cx| {
        view.making = None;
        view.run("gateway.add", window, cx);
        view.cancel_adding(window, cx);
    });
    view.read_with(cx, |view, _| assert!(view.adding.is_none()));
}

#[gpui_kit::test]
fn an_address_that_isnt_one_is_said_and_nothing_is_asked_of_openshell(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update_in(cx, |view, window, cx| {
        view.run("gateway.add", window, cx);
        view.adding
            .as_ref()
            .unwrap()
            .type_address("ftp://os.example.com", window, cx);
        view.add_gateway(window, cx);
    });
    view.read_with(cx, |view, _| {
        let said = view.adding.as_ref().unwrap().failure().unwrap();
        assert!(said.contains("https://"), "{said}");
    });
}

#[gpui_kit::test]
fn an_oidc_gateway_is_refused_without_an_issuer_that_is_https_and_nothing_is_asked_of_openshell(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    let failure = |view: &Entity<MainView>, cx: &mut VisualTestContext| {
        view.read_with(cx, |view, _| {
            view.adding.as_ref().unwrap().failure().map(str::to_string)
        })
    };
    view.update_in(cx, |view, window, cx| {
        view.run("gateway.add", window, cx);
        view.adding
            .as_ref()
            .unwrap()
            .type_address("https://os.example.com", window, cx);
        view.use_oidc(true, cx);
        view.add_gateway(window, cx);
    });
    assert!(failure(&view, cx).unwrap().contains("OIDC issuer"));
    view.update_in(cx, |view, window, cx| {
        view.adding
            .as_ref()
            .unwrap()
            .type_issuer("http://login.example.com", window, cx);
        view.add_gateway(window, cx);
    });
    assert!(failure(&view, cx).unwrap().contains("starts with https://"));
}

#[gpui_kit::test]
fn the_gateway_openshell_runs_on_this_mac_is_not_removed_from_here(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update(cx, |view, _| view.gateways_listed(Ok(gateways())));
    view.update_in(cx, |view, window, cx| {
        view.confirm_remove_gateway("openshell".to_string(), window, cx);
    });
    view.read_with(cx, |view, _| {
        let said = view.failed.as_deref().unwrap();
        assert!(said.contains("runs on this Mac"), "{said}");
    });
    // A gateway that isn't registered is nothing to remove.
    view.update_in(cx, |view, window, cx| {
        view.failed = None;
        view.confirm_remove_gateway("nowhere".to_string(), window, cx);
    });
    view.read_with(cx, |view, _| assert_eq!(view.failed, None));
}

#[test]
fn a_gateway_is_placed_on_this_mac_or_by_the_host_of_its_endpoint() {
    let mut all = gateways();
    assert_eq!(gateway_where(&all[0]), "This Mac");
    all[1].endpoint = "https://os.example.com:8443/grpc".to_string();
    assert_eq!(gateway_where(&all[1]), "os.example.com:8443");
    all[2].endpoint = "ssh://sam@10.0.0.5:8080".to_string();
    assert_eq!(gateway_where(&all[2]), "sam@10.0.0.5:8080");
    all[2].endpoint = "10.0.0.6".to_string();
    assert_eq!(gateway_where(&all[2]), "10.0.0.6", "no scheme to take off");
}

#[gpui_kit::test]
fn choosing_a_gateway_by_name_shows_it_and_a_name_that_isnt_registered_does_nothing(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    view.update(cx, |view, _| view.gateways_listed(Ok(gateways())));
    listed(&view, vec![sandbox("alpha", Phase::Ready)], cx);
    view.update_in(cx, |view, window, cx| {
        view.show_gateway("openshell", window, cx);
    });
    view.read_with(cx, |view, _| {
        assert!(
            matches!(view.listing(), Listing::Listed(_)),
            "the gateway that's shown already isn't asked again"
        );
        assert_eq!(
            view.selected.as_ref().map(|key| key.name.as_str()),
            Some("alpha")
        );
    });
    view.update_in(cx, |view, window, cx| {
        view.show_gateway("nowhere", window, cx);
    });
    assert_eq!(shown(&view, cx).as_deref(), Some("openshell"));
    view.update_in(cx, |view, window, cx| {
        view.show_gateway("k8s-home", window, cx);
    });
    assert_eq!(shown(&view, cx).as_deref(), Some("k8s-home"));
    view.read_with(cx, |view, _| {
        assert!(matches!(view.listing(), Listing::Loading));
        assert_eq!(view.selected, None);
    });
}

#[gpui_kit::test]
fn set_up_microvm_gateway_shows_its_form_in_place_of_the_others_and_chooses_each_sandboxs_size(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    view.update_in(cx, |view, window, cx| {
        view.run("gateway.add", window, cx);
        view.run("gateway.microvm", window, cx);
    });
    view.read_with(cx, |view, _| {
        assert!(view.microvm.is_some() && view.adding.is_none());
        let form = view.microvm.as_ref().unwrap();
        assert_eq!(form.size(), playpen_core::MicroVmSize::default());
        assert_eq!(form.failure(), None);
    });
    view.update(cx, |view, cx| {
        view.choose_microvm_size(microvm::SizePart::Cpus, 8, cx);
        view.choose_microvm_size(microvm::SizePart::Memory, 2, cx);
        view.choose_microvm_size(microvm::SizePart::Disk, 32, cx);
    });
    view.read_with(cx, |view, _| {
        let size = view.microvm.as_ref().unwrap().size();
        assert_eq!((size.cpus, size.memory_gib, size.disk_gib), (8, 2, 32));
    });
    // New Agent… takes its place.
    cx.simulate_keystrokes("cmd-n");
    view.read_with(cx, |view, _| {
        assert!(view.microvm.is_none() && view.making.is_some());
    });
    view.update_in(cx, |view, window, cx| {
        view.making = None;
        view.run("gateway.microvm", window, cx);
        view.cancel_microvm(window, cx);
    });
    view.read_with(cx, |view, _| assert!(view.microvm.is_none()));
}

#[gpui_kit::test]
fn a_sandbox_with_network_requests_waiting_says_so_and_decided_ones_dont_count(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    listed(&view, vec![sandbox("alpha", Phase::Ready)], cx);
    view.update(cx, |view, _| {
        let key = view.selected.clone().expect("alpha is chosen");
        let mut requests = super::requests::stand_in_requests();
        view.give_requests(key.clone(), requests.clone());
        assert_eq!(
            view.waiting_words(&key).as_deref(),
            Some("2 requests waiting")
        );
        requests.retain(|request| !request.is_pending());
        view.give_requests(key.clone(), requests);
        assert_eq!(view.waiting_words(&key), None);
    });
}

#[gpui_kit::test]
fn recent_activity_shows_the_newest_first_and_can_show_only_what_was_stopped(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    listed(&view, vec![sandbox("alpha", Phase::Ready)], cx);
    view.update(cx, |view, cx| {
        view.give_activity(super::activity::stand_in_activity());
        let all = view.activity_shown();
        assert_eq!(all.len(), 4);
        assert!(all[0].1 > all[3].1, "newest first");
        view.show_only_blocked(true, cx);
        let blocked = view.activity_shown();
        assert_eq!(blocked.len(), 2);
        assert!(blocked.iter().all(|(allowed, _)| !allowed));
    });
}

#[gpui_kit::test]
fn a_forwarded_port_shows_only_on_its_own_sandbox_and_gateway(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    listed(&view, vec![sandbox("alpha", Phase::Ready)], cx);
    view.update(cx, |view, _| {
        let key = view.selected.clone().expect("alpha is chosen");
        view.give_forward(playpen_core::Forward {
            gateway: "elsewhere".to_string(),
            sandbox: "alpha".to_string(),
            port: 8080,
        });
        assert!(
            view.forwards_of(&key).is_empty(),
            "alpha on another gateway"
        );
        view.ports_give_stand_ins(&key);
        assert_eq!(view.forwards_of(&key), [3000, 5173]);
    });
}

#[gpui_kit::test]
fn escape_closes_an_open_form_even_while_a_field_has_focus(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    listed(
        &view,
        vec![
            sandbox("alpha", Phase::Ready),
            sandbox("beta", Phase::Ready),
        ],
        cx,
    );
    // New Agent focuses its Name field; Escape must still close the form and return arrow navigation.
    cx.simulate_keystrokes("cmd-n");
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(view.making.is_some()));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(view.making.is_none()));
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&view, cx).as_deref(), Some("beta"));

    // Add Gateway focuses its Address field; Escape closes it too.
    view.update_in(cx, |view, window, cx| {
        view.run("gateway.add", window, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(view.adding.is_some()));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(view.adding.is_none()));
}

#[gpui_kit::test]
fn deleting_a_kit_shows_an_undo_toast_that_restores_it(cx: &mut TestAppContext) {
    use gpui_kit::Modifiers;
    use std::time::Duration;

    let (view, cx) = open(cx);
    view.update_in(cx, |view, window, cx| {
        view.kits.kits.kits.push(playpen_core::Kit {
            name: "work".to_string(),
            setup_folder: None,
            model_key: Some("claude-work".to_string()),
            services: vec!["github".to_string()],
            variables: Vec::new(),
        });
        view.show_page(Page::Kits, window, cx);
        view.select_kit("work".to_string(), window, cx);
        view.delete_kit(window, cx);
    });
    cx.background_executor
        .advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(view.kits.kits.kits.is_empty());
    });
    let button = cx
        .debug_bounds("toast-action")
        .expect("the Undo button on the toast");
    cx.simulate_click(button.center(), Modifiers::none());
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.kits.kits.kits.len(), 1);
        assert_eq!(view.kits.kits.kits[0].name, "work");
        assert_eq!(view.kits.selected.as_deref(), Some("work"));
    });
}

#[gpui_kit::test]
fn attention_count_tracks_pending_network_requests_on_ready_sandboxes(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    listed(&view, vec![sandbox("alpha", Phase::Ready)], cx);
    view.update(cx, |view, _| {
        assert_eq!(view.attention_count(), 0);
        let key = view.selected.clone().expect("alpha is chosen");
        view.give_requests(key, super::requests::stand_in_requests());
        assert_eq!(view.attention_count(), 2);
    });
}

#[gpui_kit::test]
fn attach_autocomplete_lists_unattached_services_kits_and_new_service(cx: &mut TestAppContext) {
    use gpui_kit::component::select::SelectItem as _;

    let (view, cx) = open(cx);
    listed(&view, vec![sandbox("alpha", Phase::Ready)], cx);
    view.update(cx, |view, _| {
        let key = view.selected.clone().expect("alpha is chosen");
        view.give_stand_in_services(&key);
        view.kits.kits.kits.push(playpen_core::Kit {
            name: "work".to_string(),
            setup_folder: None,
            model_key: None,
            services: vec!["github".to_string(), "linear".to_string()],
            variables: Vec::new(),
        });
        let titles: Vec<String> = view
            .attach_choices()
            .into_iter()
            .map(|choice| choice.title().to_string())
            .collect();
        assert_eq!(
            titles,
            [
                "linear · api.linear.app",
                "sentry · sentry.io",
                "Kit: work",
                "+ New Service…",
            ]
        );
    });
}

#[gpui_kit::test]
fn a_custom_agent_is_added_from_its_form_and_new_agent_offers_only_the_keys_it_reads(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    view.update_in(cx, |view, window, cx| {
        view.run("agent.new-custom", window, cx);
        view.fill_custom_agent(
            [
                "aider",
                "ghcr.io/me/aider:1.2",
                "aider",
                "",
                "/usr/local/bin/python3.12",
            ],
            window,
            cx,
        );
        view.run_save_custom_agent(window, cx);
    });
    view.read_with(cx, |view, _| {
        assert!(view.agents.form.is_none(), "saved and closed");
        assert_eq!(
            view.agents.selected,
            agents::Chosen::Custom("aider".to_string())
        );
        assert!(view.agents.custom.find("aider").is_some());
    });
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
                    name: "openrouter".to_string(),
                    kind: "playpen-openrouter".to_string(),
                },
            ]),
            cx,
        );
        let aider = view.harnesses().pop().expect("the custom agent is offered");
        view.choose_agent(aider, window, cx);
    });
    view.read_with(cx, |view, cx| {
        let making = view.making.as_ref().expect("New Agent… is showing");
        assert_eq!(making.chosen_provider(), Some("openrouter"));
        assert_eq!(making.name_typed(cx), "aider");
    });
}

#[gpui_kit::test]
fn a_custom_agent_named_like_one_of_playpens_is_refused_in_its_form(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update_in(cx, |view, window, cx| {
        view.run("agent.new-custom", window, cx);
        view.fill_custom_agent(
            ["pi", "ghcr.io/me/pi:1", "pi", "", "/usr/local/bin/node"],
            window,
            cx,
        );
        view.run_save_custom_agent(window, cx);
    });
    view.read_with(cx, |view, _| {
        assert!(view.agents.form.is_some(), "still open, saying why");
        assert!(view.agents.custom.agents.is_empty());
    });
}

#[gpui_kit::test]
fn the_welcome_offers_setting_up_which_starts_by_asking_for_the_model(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update_in(cx, |view, window, cx| view.run("help.welcome", window, cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.guide_step_shown(), Some(guide::GuideStep::Welcome));
    });
    view.update_in(cx, |view, window, cx| {
        view.pick_in_guide(guide::Pick::Setup, window, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.guide_step_shown(), Some(guide::GuideStep::ChooseAgent));
    });
}

#[gpui_kit::test]
fn the_tour_shows_each_place_and_ends_offering_to_set_up(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    view.update_in(cx, |view, window, cx| view.run("help.tour", window, cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.guide_step_shown(), Some(guide::GuideStep::Places));
    });
    for _ in 0..4 {
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
    }
    view.read_with(cx, |view, _| {
        assert_eq!(view.guide_step_shown(), Some(guide::GuideStep::Kits));
        assert_eq!(view.page, Page::Kits, "the step's place is shown");
    });
}

#[gpui_kit::test]
fn setting_up_on_docker_steps_aside_until_the_gateway_answers_then_asks_for_its_key(
    cx: &mut TestAppContext,
) {
    let (view, cx) = open(cx);
    view.update_in(cx, |view, window, cx| view.run("help.setup", window, cx));
    cx.run_until_parked();
    view.update_in(cx, |view, window, cx| {
        view.pick_in_guide(guide::Pick::Gateway(guide::Place::Docker), window, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.guide_step_shown(), Some(guide::GuideStep::Gateway));
        assert!(view.guide_banner_shown(), "the step waits in the corner");
    });
    listed(&view, vec![sandbox("claude-patchbay", Phase::Ready)], cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.guide_step_shown(), Some(guide::GuideStep::ChooseKey));
        assert!(!view.guide_banner_shown());
    });
}
