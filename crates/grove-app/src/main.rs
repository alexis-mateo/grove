mod branches;
mod builtin_glyphs;
mod claude;
mod fonts;
mod grove_view;
mod images;
mod keys;
mod list;
mod markdown;
mod marquee;
mod mouse;
mod native_view;
mod profiles;
mod settings;
mod shortcuts;
mod sidebar;
mod tab_bar;
mod terminal_view;
mod text_field;
mod theme;
mod ui;
mod usage;
mod window_frame;
mod worktrees;

use std::path::PathBuf;

use anyhow::Result;
use gpui::{App, AppContext, Application, Bounds, WindowBounds, WindowDecorations, WindowOptions, px, size};
use grove_core::{Config, Paths};

use crate::grove_view::GroveView;

fn main() -> Result<()> {
    // Appelé par les hooks de Claude Code : répondre vite, sans rien initialiser d'autre.
    match std::env::args().nth(1).as_deref() {
        Some("hook") => return exit(grove_claude::run_hook()),
        Some("statusline") => return exit(grove_claude::run_statusline()),
        // Serveur MCP des demandes de permission de la vue native, lancé par Claude Code.
        Some("mcp-permission") => return exit(grove_claude::run_mcp_server()),
        _ => {}
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,grove=info")).init();

    // `grove [dossier]` : ajoute le dossier comme projet et l'ouvre.
    let open = std::env::args_os().nth(1).map(PathBuf::from);
    let paths = Paths::from_env()?;
    let config = Config::load(&paths.config_file())?;

    Application::new().with_assets(ui::Assets).run(move |cx: &mut App| {
        cx.bind_keys(grove_view::key_bindings());
        let bounds = Bounds::centered(None, size(px(1200.0), px(760.0)), cx);
        let window = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("grove".into()),
                // GNOME ne dessine pas les décorations des clients Wayland :
                // on les dessine nous-mêmes (barre de titre, ombre, bords).
                window_decorations: Some(WindowDecorations::Client),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| GroveView::new(paths, config, open, window, cx)),
        );
        match window {
            Ok(window) => {
                // Fermer la fenêtre quitte l'application.
                let _ = window.update(cx, |_, window, cx| {
                    window.on_window_should_close(cx, |_, cx| {
                        cx.quit();
                        true
                    })
                });
            }
            Err(err) => {
                log::error!("ouverture de la fenêtre : {err:#}");
                cx.quit();
            }
        }
    });
    Ok(())
}

fn exit(code: std::process::ExitCode) -> Result<()> {
    std::process::exit(if code == std::process::ExitCode::SUCCESS { 0 } else { 1 })
}
