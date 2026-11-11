use egui::{Color32, FontFamily, FontId, Stroke, TextStyle};

#[allow(dead_code)]
pub mod colors {
    use egui::Color32;

    pub const BG_APP: Color32 = Color32::from_rgb(19, 19, 22); // Note area
    pub const BG_SIDEBAR: Color32 = Color32::from_rgb(15, 15, 18); // Sidebar, a touch darker
    pub const BG_HEADER: Color32 = Color32::from_rgb(15, 15, 18);
    pub const BG_CARD: Color32 = Color32::from_rgb(28, 28, 33); // Dialogs, popups
    pub const BG_CARD_HOVER: Color32 = Color32::from_rgb(26, 26, 31);
    pub const BG_CARD_ACTIVE: Color32 = Color32::from_rgb(32, 36, 52); // Selected sidebar row
    pub const BG_CODE: Color32 = Color32::from_rgb(13, 13, 16);
    pub const BG_INPUT: Color32 = Color32::from_rgb(24, 24, 28);

    /// Panel fills when the window has the blurred glass background: see-through so the blur shows.
    pub const GLASS_APP: Color32 = Color32::from_rgba_unmultiplied_const(22, 22, 26, 150);
    pub const GLASS_SIDEBAR: Color32 = Color32::from_rgba_unmultiplied_const(26, 26, 30, 130);
    pub const GLASS_INPUT: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 12);
    /// Sidebar rows: selected and hovered highlights.
    pub const ROW_SELECTED: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 22);
    pub const ROW_HOVER: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 9);

    pub const ACCENT: Color32 = Color32::from_rgb(85, 130, 240);
    pub const ACCENT_MUTED: Color32 = Color32::from_rgb(30, 40, 72);

    pub const BORDER: Color32 = Color32::from_rgb(28, 28, 34);
    pub const BORDER_LIGHT: Color32 = Color32::from_rgb(40, 40, 48);
    /// Divider lines (rules, heading underlines, table grid). Translucent white so they show on
    /// any background, including the blurred glass.
    pub const LINE: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 24);

    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(232, 232, 238);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(150, 152, 162);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(92, 94, 106);
    pub const TEXT_ACCENT: Color32 = Color32::from_rgb(115, 160, 255);

    pub const SUCCESS: Color32 = Color32::from_rgb(65, 195, 110);
    pub const WARNING: Color32 = Color32::from_rgb(235, 160, 40);
    pub const DANGER: Color32 = Color32::from_rgb(235, 85, 85);
}

/// Primary fonts: the macOS system font (San Francisco) for text and SF Mono for code.
#[cfg(target_os = "macos")]
const PRIMARY_FONTS: &[(&str, &str, FontFamily)] = &[
    ("sf", "/System/Library/Fonts/SFNS.ttf", FontFamily::Proportional),
    ("sf_mono", "/System/Library/Fonts/SFNSMono.ttf", FontFamily::Monospace),
];
#[cfg(not(target_os = "macos"))]
const PRIMARY_FONTS: &[(&str, &str, FontFamily)] = &[];

/// Fallbacks for characters the primary fonts lack (CJK, rare symbols, ...). Missing files are skipped.
#[cfg(target_os = "macos")]
const FALLBACK_FONTS: &[(&str, &str)] = &[
    ("menlo", "/System/Library/Fonts/Menlo.ttc"),
    ("apple_symbols", "/System/Library/Fonts/Apple Symbols.ttf"),
    ("arial_unicode", "/System/Library/Fonts/Supplemental/Arial Unicode.ttf"),
];
#[cfg(not(target_os = "macos"))]
const FALLBACK_FONTS: &[(&str, &str)] = &[
    ("dejavu", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
    ("noto", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"),
    ("segoe", "C:\\Windows\\Fonts\\segoeui.ttf"),
];

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut load = |name: &str, path: &str| -> bool {
        match std::fs::read(path) {
            Ok(bytes) => {
                fonts.font_data.insert(name.to_string(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    let mut primary = Vec::new();
    for (name, path, family) in PRIMARY_FONTS {
        if load(name, path) {
            primary.push((*name, family.clone()));
        }
    }
    let mut fallback = Vec::new();
    for (name, path) in FALLBACK_FONTS {
        if load(name, path) {
            fallback.push(*name);
        }
    }

    for (name, family) in &primary {
        // System font first; egui's built-in fonts (incl. emoji) stay behind it as fallbacks.
        fonts.families.entry(family.clone()).or_default().insert(0, name.to_string());
    }
    // Each family can also borrow glyphs from the other family's system font, then the fallbacks.
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        for (name, _) in &primary {
            if !list.iter().any(|n| n == name) {
                list.push(name.to_string());
            }
        }
        list.extend(fallback.iter().map(|n| n.to_string()));
    }
    ctx.set_fonts(fonts);
}

pub fn configure_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    style.text_styles = [
        (TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        (TextStyle::Button, FontId::new(12.5, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
    ]
    .into();

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = colors::BG_APP;
    visuals.window_fill = colors::BG_CARD;
    visuals.window_stroke = Stroke::new(1.0, colors::BORDER_LIGHT);
    visuals.window_corner_radius = egui::CornerRadius::same(10);
    visuals.window_shadow = egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(110) };
    visuals.popup_shadow = egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(90) };
    visuals.menu_corner_radius = egui::CornerRadius::same(8);

    // Quiet widgets: flat until hovered, no outlines at rest.
    let radius = egui::CornerRadius::same(6);
    visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
    visuals.widgets.noninteractive.corner_radius = radius;

    visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, colors::TEXT_SECONDARY);
    visuals.widgets.inactive.corner_radius = radius;

    visuals.widgets.hovered.bg_fill = Color32::from_white_alpha(10);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_white_alpha(10);
    visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, colors::TEXT_PRIMARY);
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.hovered.expansion = 0.0;

    visuals.widgets.active.bg_fill = Color32::from_white_alpha(16);
    visuals.widgets.active.weak_bg_fill = Color32::from_white_alpha(16);
    visuals.widgets.active.bg_stroke = Stroke::NONE;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, colors::TEXT_PRIMARY);
    visuals.widgets.active.corner_radius = radius;
    visuals.widgets.active.expansion = 0.0;

    visuals.widgets.open.bg_fill = colors::BG_CARD_HOVER;
    visuals.widgets.open.corner_radius = radius;

    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(85, 130, 240, 90);
    visuals.selection.stroke = Stroke::new(1.0, colors::TEXT_PRIMARY);
    visuals.text_cursor.stroke = Stroke::new(1.6, colors::ACCENT);

    visuals.extreme_bg_color = colors::BG_INPUT;
    visuals.faint_bg_color = colors::BG_CARD_HOVER;

    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.scroll = egui::style::ScrollStyle {
        floating: true,
        bar_width: 8.0,
        floating_width: 6.0,
        floating_allocated_width: 0.0,
        foreground_color: true,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.15,
        dormant_handle_opacity: 0.35,
        active_handle_opacity: 0.65,
        interact_handle_opacity: 0.95,
        bar_inner_margin: 2.0,
        bar_outer_margin: 2.0,
        handle_min_length: 24.0,
    };
    style.interaction.selectable_labels = false;

    ctx.set_style(style);
}
