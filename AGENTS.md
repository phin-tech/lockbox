# Playpen

Playpen is a macOS app in Rust. `crates/core` (`playpen-core`) holds everything that isn't UI and is tested
headless; `crates/app` is the GPUI front end. Read `CONTEXT.md` before naming anything.

## Where things are

- `crates/app/src/commands.rs`: every Command, in `COMMANDS`, and the ones any window runs. `view/mod.rs` runs the main
  view's own and hands it the rest. `shortcuts.rs`: keys. `menus.rs`: the menu bar, which the command palette
  (`palette.rs`, ⌘K, Typecase's, shown over the main view) lists too. Tests hold every menu item and key to `COMMANDS`, and every Command to having a case in
  a `run` and a place in a menu or a key.
- `settings.rs`: the Settings window (⌘,), Typecase's frame with a section for each part, over
  `playpen_core::Settings`, a TOML file in Application Support.
  `restore.rs` puts the main window back where it was (`state.toml`, beside the settings).
- `language.rs` and `locales/app.toml`: the language, and translations keyed by the English text.
- `login_item.rs`: Open at login, in Settings.
- `menu_bar.rs`: the menu bar extra and its shortcut (`hotkey` in the settings file).
- `view/guide.rs`: the welcome at a first launch (remembered in `state.toml`), the tour of the window and setting up,
  Typecase tours with cards of Playpen's own. A step that waits on the person steps the tour aside for a card in the
  window's corner, and the window says when it's done (`guide_done`).
- `about.rs`: About, and what Keys (⌘/), Typecase's sheet over the main view, lists: every key by its menu. `logging.rs`: `tracing`, to stderr and
  `~/Library/Logs/tech.phin.playpen/`; `crash.rs` writes a report there on a panic and says so at the next launch.
  `theme.rs`: the look, Typecase's themes, which follow the appearance, theme and text size set there.
- `update.rs`: Sparkle, which only the released app carries; Check for Updates… and Settings ask it.
- `view/`: the window. `sandboxes.rs` is the sidebar (each gateway, its sandboxes nested under it) and acts on the
  chosen sandbox, `gateways.rs` adds, signs in to and removes a gateway, `images.rs` is Gateway ▸ Image Source…,
  `making.rs` is New Agent…. `providers.rs` is Settings ▸ Providers, which acts on the gateway in focus. Every call to
  OpenShell or Docker runs off the window's thread.
- `crates/core/src/openshell/`: the `openshell` command, whose JSON it reads, and which gateway it goes to; `register.rs`
  there is `gateway add`, `login` and `remove`. `agent/` each agent's image, policy and how a sandbox is made for it;
  `image.rs` where an image comes from (this Mac's Docker, a registry, or the person's own) and `docker.rs` builds and
  pushes it; `herdr.rs` opens a sandbox's agent in a Herdr terminal, labelled `herdr.space`/`herdr.tab` so the Herdr app
  shows it. `just openshell-test <provider>` makes a real sandbox.
- Gateways: OpenShell keeps the list (`openshell gateway list`), Playpen keeps none. What Playpen keeps for one is a
  `[gateways.<name>]` section of the settings file: where its images come from.
- `crates/cli`: the command line, which the app's own binary answers before any window opens: the services catalog,
  gateways, sandboxes (start, stop, restart, delete, attach and detach services) and providers, each with `--json` and
  `--gateway`. Errors go to stderr; a `--` option or a near miss of a command is an error, not a window. A new CLI
  command takes its place in `parse`, `USAGE` and the tests there.
- `DEVELOPMENT.md`: building, checking, CI, releasing.

## Done

The change is done when these pass, and your report names any you did not run:

- `just check`, always: formatting, clippy and every test.
- `just lint` for a new dependency: its licence has to be in `deny.toml`.
- `just shot`, looked at in light and dark, for anything visible.
- `just release-check` for changes to `release.toml` or `scripts/release`.

Write test names as sentences saying what the app does. Comments say why, in plain sentences.

## Seeing the GUI

The maintainer works alongside you, so GUI runs stay hidden and leave focus alone. Ask before any run that needs the
foreground.

- `just shot <name> <appearance>` saves a frame to `target/shots/<name>.png`. `PLAYPEN_KEYS` drives it first:
  keys, `click:X,Y` (or `cmd-click:X,Y`) and `wait`, separated by spaces. The frame is the newest window, so
  `PLAYPEN_KEYS="cmd-, wait"` shows Settings and `cmd-k` the palette.
- The frame is at 2x: divide its pixel coordinates by two for `click:`.
- Shots read and write no settings, so yours are safe. `PLAYPEN_SETTINGS=<file>` gives them some, and
  `PLAYPEN_WIDTH` sets the main window's width.
- Hidden frames don't show a selection highlight.
- Check layout at the window's real size. A larger frame hides clipping.

## Gotchas

- A call goes to a gateway by `OPENSHELL_GATEWAY`, set by `OpenShell::on`, not by `--gateway`, so it reaches a shell line
  too. `openshell gateway list` is asked with none named: it marks the named one active, which reorders the list.
- A sandbox is identified by its gateway and its name (`SandboxKey`); two gateways can each hold one of a name. Herdr knows
  it by its name on the local Docker gateway and by `gateway/name` on any other. Never key a sandbox by name alone.
- `openshell gateway add` makes the new gateway the active one, and `gateway remove` leaves its folder, with the
  certificates, behind. `OpenShell::add_gateway` and `remove_gateway` put the first right and clean up the second; a test
  or a shot that registers a gateway has to remove it again and leave the active one as it was.
- A test must never reach a real `openshell` that changes something. Tests that do are `#[ignore]` and say what they touch
  (`tests/gateways.rs` registers a gateway; `tests/openshell.rs` makes a sandbox on `PLAYPEN_TEST_GATEWAY`).
- `just shot` runs against OpenShell's real configuration, so a shot shows the gateways and sandboxes that are really
  there.
- A Kubernetes gateway takes sandbox names of up to 19 characters, Docker's 63, and its nodes must support
  `supplementalGroupsPolicy` (OrbStack's don't); `docs/adr/0001-*` has what trying a real cluster found.

- A key bound in a view's context reaches every text field inside the view too, and takes the letter before the field
  sees it: `o` bound to a Command makes `o` impossible to type in a search box there. The main view keeps its context
  only while it has focus itself; a view of your own with single-letter keys and a text field needs the same.
- `use gpui_kit::*` in a test module shadows `#[test]` and fails with a recursion limit error. Import names from `super`
  instead.
- The kit's `Input` fills whatever height it's offered, so give it a fixed height. It also binds `tab` and `escape`, so
  overriding them takes a binding on `Parent > Input`.
- `cargo test` leaves `target/debug/playpen` stale. Run `cargo build` before trying that binary.
- Text the app shows goes through `t!("The English text")` (`rust_i18n::t`), with `%{name}` for what goes in it:
  `t!("Save the changes to “%{name}”?", name = title)`. The English is the key, and what shows until someone
  translates it. GPUI finds the Window menu by its English name, so that one stays as it is.
- Errors: `thiserror` enums in the core, where a caller may match on them; `anyhow` in the app, with `.context(…)`
  saying what was being done, shown with `{error:#}`.
- Text output, file formats and reports are tested with `insta::assert_snapshot!`; `just review` accepts a change.
- `just inspect` runs the app with the element inspector (⌥⌘I): click anything to see its layout, style and source.
- Log with `tracing::info!` and friends, not `eprintln!`, so it reaches the log file. `PLAYPEN_LOG=debug`
  shows more.
- Unsafe code is denied in the workspace (`Cargo.toml`'s lints); a module that needs the Objective-C runtime says so
  with `#![expect(unsafe_code, reason = …)]`, as `update.rs` does, and each block with a `// SAFETY:` comment.
- Colours come from Typecase's themes, chosen in Settings ▸ Appearance. Views take them from `cx.theme()`, and text
  sizes from the theme or rems, so the text size in Settings reaches them; a fixed width that holds text should grow with it too.
- A window of a new kind hands the Commands it doesn't run to `commands::run`, as Settings does, or the menus do
  nothing while it has focus.
- Anything reached with the mouse should be a Command in a menu, which puts it in the palette as well.
- macOS adds Enter Full Screen to the View menu itself, so there's no menu item for `window.fullscreen`, only ⌃⌘F.
- `gpui-kit` is pinned exactly, and its GPUI snapshot's API moves. Bump it on its own, with `rust-toolchain.toml` if it
  needs that. If the kit itself needs changing, copy the crate to `vendor/`, add it under `[patch.crates-io]`, exclude
  `vendor` from the workspace and mark each change, so it can be reapplied at the next bump.
- `release.toml`'s `[cargo]` copies one binary into the bundle, named after the package. Anything else the app needs at
  run time has to be compiled in (`include_str!`, `include_bytes!`).
- Files the app keeps (settings, state, documents) are written with `write_atomically` from the core, so a crash
  midway never leaves half a file.
- `.ci.star`'s steps get a clean environment. A step that needs a variable sets it in its `env`.
- The release tool is app-release, a separate repository pinned in `scripts/release/pyproject.toml`; change it there.

## Releases

The maintainer releases with `just ship patch|minor|major|beta`; leave releases, version bumps and pushes to them unless
asked. Every release recipe takes `--dry-run`.
