use eframe::egui::{self, Color32, FontId, TextStyle};
pub const ACCENT: Color32 = Color32::from_rgb(111, 180, 255);
pub const MUTED: Color32 = Color32::from_rgb(149, 160, 180);
pub const LINE: Color32 = Color32::from_rgb(47, 55, 69);
pub const SURFACE: Color32 = Color32::from_rgb(24, 29, 38);
pub fn configure(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\malgun.ttf") {
        fonts
            .font_data
            .insert("korean".into(), egui::FontData::from_owned(bytes).into());
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "korean".into());
    }
    ctx.set_fonts(fonts);
    ctx.set_visuals(egui::Visuals::dark());
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(13.));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(13.));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(11.));
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(18.));
    style.spacing.item_spacing = egui::vec2(8., 8.);
    style.spacing.button_padding = egui::vec2(10., 5.);
    style.spacing.interact_size = egui::vec2(28., 28.);
    style.spacing.window_margin = egui::Margin::same(16);
    style.visuals.panel_fill = Color32::from_rgb(18, 22, 29);
    style.visuals.window_fill = SURFACE;
    style.visuals.extreme_bg_color = Color32::from_rgb(14, 18, 25);
    style.visuals.faint_bg_color = Color32::from_rgb(24, 29, 38);
    style.visuals.override_text_color = Some(Color32::from_rgb(224, 231, 241));
    style.visuals.selection.bg_fill = Color32::from_rgb(36, 66, 101);
    style.visuals.selection.stroke = egui::Stroke::new(1., Color32::from_rgb(234, 244, 255));
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
    ] {
        widget.corner_radius = 6.into();
        widget.expansion = 0.;
    }
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(31, 38, 49);
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(29, 35, 45);
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1., LINE);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(44, 56, 73);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(36, 46, 61);
    style.visuals.widgets.hovered.bg_stroke =
        egui::Stroke::new(1., Color32::from_rgb(82, 119, 161));
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(43, 74, 110);
    style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1., ACCENT);
    ctx.set_style_of(egui::Theme::Dark, style);
}
pub fn count(value: usize) -> String {
    let text = value.to_string();
    let mut result = String::with_capacity(text.len() + text.len() / 3);
    for (n, c) in text.chars().enumerate() {
        if n > 0 && (text.len() - n).is_multiple_of(3) {
            result.push(',');
        }
        result.push(c);
    }
    result
}
pub fn search_mark(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    painter.circle_stroke(
        center - egui::vec2(1.5, 1.5),
        5.,
        egui::Stroke::new(1.7, color),
    );
    painter.line_segment(
        [center + egui::vec2(2., 2.), center + egui::vec2(6., 6.)],
        egui::Stroke::new(1.7, color),
    );
}
