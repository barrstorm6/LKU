use egui::{Color32, Stroke};
pub const INK: Color32 = Color32::from_rgb(17, 17, 17);
pub const GRAY: Color32 = Color32::from_rgb(119, 119, 119);
pub const GREEN: Color32 = Color32::from_rgb(22, 138, 69);
pub const RED: Color32 = Color32::from_rgb(211, 47, 47);
pub const BORDER: Color32 = Color32::from_rgb(229, 229, 229);
pub const LIGHT: Color32 = Color32::from_rgb(245, 245, 245);

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();
    style.visuals = egui::Visuals::light();
    style.visuals.override_text_color = Some(INK);
    style.visuals.panel_fill = Color32::WHITE;
    style.visuals.window_fill = Color32::WHITE;
    style.visuals.extreme_bg_color = Color32::WHITE;
    style.visuals.faint_bg_color = LIGHT;
    style.visuals.selection.bg_fill = BORDER;
    style.visuals.selection.stroke = Stroke::new(1.0, INK);
    style.visuals.window_stroke = Stroke::new(1.0, BORDER);
    style.visuals.window_shadow = egui::epaint::Shadow::NONE;
    style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
    for widget in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widget.bg_fill = LIGHT;
        widget.weak_bg_fill = LIGHT;
        widget.bg_stroke = Stroke::new(1.0, BORDER);
        widget.fg_stroke = Stroke::new(1.0, INK);
    }
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.animation_time = 0.0;
    ctx.set_style_of(egui::Theme::Light, style);
    // Windows system fonts provide Cyrillic; default fonts remain as fallback.
    #[cfg(target_os = "windows")]
    if let Some(windows) = std::env::var_os("WINDIR") {
        let font = std::path::PathBuf::from(windows)
            .join("Fonts")
            .join("segoeui.ttf");
        if let Ok(bytes) = std::fs::read(font) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("system".into(), egui::FontData::from_owned(bytes).into());
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "system".into());
            ctx.set_fonts(fonts);
        }
    }
}
