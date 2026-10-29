use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke, Visuals};

pub const BG: Color32 = Color32::from_rgb(0x0F, 0x11, 0x15);
pub const SURFACE: Color32 = Color32::from_rgb(0x16, 0x18, 0x1E);
pub const SURFACE_2: Color32 = Color32::from_rgb(0x1E, 0x21, 0x29);
pub const SURFACE_3: Color32 = Color32::from_rgb(0x27, 0x2B, 0x35);
pub const BORDER: Color32 = Color32::from_rgb(0x26, 0x29, 0x32);
pub const TEXT: Color32 = Color32::from_rgb(0xE8, 0xEA, 0xEE);
pub const MUTED: Color32 = Color32::from_rgb(0x8C, 0x92, 0x9E);
pub const FAINT: Color32 = Color32::from_rgb(0x5C, 0x62, 0x6E);
pub const ACCENT: Color32 = Color32::from_rgb(0x8B, 0x9C, 0xFF);
pub const KEEP: Color32 = Color32::from_rgb(0x3D, 0xD6, 0x8C);
pub const DELETE: Color32 = Color32::from_rgb(0xFF, 0x5D, 0x6C);
pub const WARN: Color32 = Color32::from_rgb(0xF5, 0xB7, 0x4F);

pub const RADIUS: u8 = 10;

/// Font family used for headings, when a semibold system font is available.
pub fn heading_family() -> FontFamily {
    FontFamily::Name("heading".into())
}

fn load_system_font(names: &[&str]) -> Option<Vec<u8>> {
    let dir = std::env::var_os("WINDIR")
        .map(|w| std::path::PathBuf::from(w).join("Fonts"))
        .unwrap_or_else(|| "C:\\Windows\\Fonts".into());
    names.iter().find_map(|n| std::fs::read(dir.join(n)).ok())
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    let mut heading = fonts.families[&FontFamily::Proportional].clone();
    if let Some(data) = load_system_font(&["segoeui.ttf"]) {
        fonts.font_data.insert("ui".into(), FontData::from_owned(data).into());
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "ui".into());
        heading.insert(0, "ui".into());
    }
    if let Some(data) = load_system_font(&["seguisb.ttf", "segoeuib.ttf"]) {
        fonts.font_data.insert("ui-semibold".into(), FontData::from_owned(data).into());
        heading.insert(0, "ui-semibold".into());
    }
    if let Some(data) = load_system_font(&["CascadiaMono.ttf", "consola.ttf"]) {
        fonts.font_data.insert("mono".into(), FontData::from_owned(data).into());
        fonts
            .families
            .get_mut(&FontFamily::Monospace)
            .unwrap()
            .insert(0, "mono".into());
    }
    fonts.families.insert(heading_family(), heading);
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        use egui::{FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Heading, FontId::new(22.0, heading_family())),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.interact_size.y = 28.0;
        style.spacing.scroll = egui::style::ScrollStyle::thin();

        let mut v = Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.extreme_bg_color = SURFACE;
        v.faint_bg_color = SURFACE_2;
        v.code_bg_color = SURFACE_2;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(RADIUS);
        v.menu_corner_radius = CornerRadius::same(RADIUS);
        v.override_text_color = Some(TEXT);
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.window_shadow = egui::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(90),
        };
        v.popup_shadow = v.window_shadow;

        let radius = CornerRadius::same(8);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = SURFACE;
        w.noninteractive.weak_bg_fill = SURFACE;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
        w.noninteractive.corner_radius = radius;

        w.inactive.bg_fill = SURFACE_2;
        w.inactive.weak_bg_fill = SURFACE_2;
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = radius;

        w.hovered.bg_fill = SURFACE_3;
        w.hovered.weak_bg_fill = SURFACE_3;
        w.hovered.bg_stroke = Stroke::new(1.0, BORDER);
        w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = SURFACE_3;
        w.active.weak_bg_fill = SURFACE_3;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.0, TEXT);
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;

        w.open = w.active;
        style.visuals = v;
    });
}
