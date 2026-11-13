// Sem janela de console preta ao abrir a versão final no Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod anim;
mod app;
mod backend;
mod platform;
mod updater;

use eframe::egui;

fn main() -> eframe::Result {
    // A janela é um retângulo invisível no topo da tela; a ilha é desenhada
    // dentro dele. Assim ela pode crescer e encolher sem redimensionar a janela.
    let viewport = egui::ViewportBuilder::default()
        .with_title("Islandbar")
        .with_inner_size(app::WINDOW_SIZE)
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_resizable(false)
        .with_taskbar(false);

    // Aberta pelo Windows no login: vai direto, sem perguntar nada.
    // Depois de uma atualização automática também, mas aí mostra um aviso.
    let args: Vec<String> = std::env::args().collect();
    let updated = args.iter().any(|arg| arg == updater::UPDATED_ARG);
    let show_welcome = !updated && !args.iter().any(|arg| arg == platform::AUTOSTART_ARG);

    platform::take_over_single_instance();
    updater::start();

    let options = eframe::NativeOptions { viewport, ..Default::default() };

    eframe::run_native(
        "Islandbar",
        options,
        Box::new(move |cc| Ok(Box::new(app::IslandApp::new(cc, show_welcome, updated)))),
    )
}
