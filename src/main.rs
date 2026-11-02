mod app;
mod markdown;
mod theme;
mod vault;
mod wiki_links;

use app::VaultApp;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([680.0, 450.0])
            .with_title("Vault")
            // Our icon for the Dock; otherwise eframe shows its default "e" icon at runtime.
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).expect("valid icon png"),
            )
            // Seamless window: no title bar, traffic lights float over the content.
            .with_fullsize_content_view(true)
            .with_title_shown(false)
            .with_titlebar_shown(false)
            // Transparent so the macOS blur (vibrancy) behind the window shows through.
            .with_transparent(true)
            .with_visible(true)
            .with_active(true),
        ..Default::default()
    };

    eframe::run_native(
        "Vault",
        native_options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            theme::configure_theme(&cc.egui_ctx);
            Ok(Box::new(VaultApp::new(cc)))
        }),
    )
}
