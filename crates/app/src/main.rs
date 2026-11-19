//! The GPUI front end. What the app does lives in `lockbox_core`.

mod about;
mod commands;
mod crash;
mod language;
mod logging;
mod login_item;
mod menu_bar;
mod menus;
mod palette;
mod restore;
#[cfg(feature = "screenshot")]
mod screenshot;
mod settings;
mod shortcuts;
mod theme;
mod update;
mod view;

use gpui_kit::assets::AllAssets;

// What the app shows in other languages, keyed by its English text: locales/app.toml.
rust_i18n::i18n!("locales", fallback = "en");

fn main() {
    // A command line it knows runs here, headless, and never opens a window.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(command) = lockbox_cli::parse(&args) {
        let status = command.run(&mut std::io::stdout()).unwrap_or(1);
        std::process::exit(status);
    }
    let _log = logging::init();
    crash::install();
    language::choose();
    let app = gpui_kit::application().with_assets(AllAssets);
    app.run(move |cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        settings::init(cx);
        restore::init(cx);
        cx.bind_keys(shortcuts::bindings());
        // The element inspector (⌥⌘I), in `just inspect` builds only.
        #[cfg(feature = "devtools")]
        gpui_devtools::init(cx);
        // After the keys are bound, since the menus show them.
        cx.set_menus(menus::menus());
        // Not while capturing frames, which must leave nothing behind.
        if !cfg!(feature = "screenshot") {
            menu_bar::start(cx);
        }
        // Not while capturing frames, which must leave nothing behind and open nothing of their own.
        if !cfg!(feature = "screenshot") {
            update::start();
        }
        let first = view::open(true, cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        #[cfg(not(feature = "screenshot"))]
        cx.activate(true);
        if let Some(first) = first {
            let _ = first.update(cx, |_, window, cx| crash::tell_if_crashed(window, cx));
        }
        #[cfg(feature = "screenshot")]
        screenshot::capture(cx);
    });
}
