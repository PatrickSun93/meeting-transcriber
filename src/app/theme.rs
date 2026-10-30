//! The look of the app: colours, spacing and text sizes for both the dark
//! and the light system theme, plus small painted widgets shared by pages.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Response, RichText, Sense, Stroke, TextStyle,
    Theme, Ui, Vec2, Visuals,
};

use crate::transcript::Speaker;

pub const ME_COLOR: Color32 = Color32::from_rgb(64, 132, 238);
pub const OTHERS_COLOR: Color32 = Color32::from_rgb(232, 142, 40);
pub const RECORD_RED: Color32 = Color32::from_rgb(228, 58, 52);
pub const READY_GREEN: Color32 = Color32::from_rgb(52, 168, 90);
/// Behind find-in-transcript results.
pub const FIND_HIGHLIGHT: Color32 = Color32::from_rgb(255, 201, 40);

/// Rounding for buttons and inputs; cards are a little rounder.
const WIDGET_RADIUS: u8 = 7;
const CARD_RADIUS: u8 = 10;

pub fn speaker_color(s: Speaker) -> Color32 {
    match s {
        Speaker::Me => ME_COLOR,
        Speaker::Others => OTHERS_COLOR,
    }
}

/// Colours that differ between dark and light mode.
struct Palette {
    accent: Color32,
    panel: Color32,
    sidebar: Color32,
    card: Color32,
    card_stroke: Color32,
    button: Color32,
    button_hover: Color32,
    extreme: Color32,
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            accent: Color32::from_rgb(122, 124, 255),
            panel: Color32::from_rgb(28, 28, 32),
            sidebar: Color32::from_rgb(21, 21, 25),
            card: Color32::from_rgb(37, 37, 43),
            card_stroke: Color32::from_rgb(50, 50, 58),
            button: Color32::from_rgb(52, 52, 61),
            button_hover: Color32::from_rgb(64, 64, 75),
            extreme: Color32::from_rgb(18, 18, 22),
        }
    } else {
        Palette {
            accent: Color32::from_rgb(84, 80, 230),
            panel: Color32::from_rgb(245, 245, 248),
            sidebar: Color32::from_rgb(234, 234, 239),
            card: Color32::WHITE,
            card_stroke: Color32::from_rgb(224, 224, 231),
            button: Color32::from_rgb(232, 232, 238),
            button_hover: Color32::from_rgb(220, 220, 229),
            extreme: Color32::WHITE,
        }
    }
}

/// The accent colour for the current theme.
pub fn accent(ui: &Ui) -> Color32 {
    palette(ui.visuals().dark_mode).accent
}

/// Styles both themes; egui keeps following the system's dark/light setting.
pub fn apply(ctx: &egui::Context) {
    install_fallback_fonts(ctx);
    for theme in [Theme::Dark, Theme::Light] {
        ctx.style_mut_of(theme, |style| {
            style.text_styles = [
                (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
                (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
                (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
                (TextStyle::Heading, FontId::new(21.0, FontFamily::Proportional)),
                (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
            ]
            .into();
            let spacing = &mut style.spacing;
            spacing.item_spacing = Vec2::new(8.0, 7.0);
            spacing.button_padding = Vec2::new(11.0, 5.0);
            spacing.interact_size.y = 26.0;
            spacing.menu_margin = Margin::same(8);
            spacing.window_margin = Margin::same(12);
            spacing.combo_width = 180.0;
            style_visuals(&mut style.visuals, theme == Theme::Dark);
        });
    }
}

fn style_visuals(v: &mut Visuals, dark: bool) {
    let p = palette(dark);
    v.panel_fill = p.panel;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.card_stroke);
    v.window_corner_radius = CornerRadius::same(CARD_RADIUS);
    v.menu_corner_radius = CornerRadius::same(WIDGET_RADIUS);
    v.extreme_bg_color = p.extreme;
    v.faint_bg_color = p.card;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(if dark { 0.45 } else { 0.3 });
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.error_fg_color = RECORD_RED;
    v.slider_trailing_fill = true;

    let w = &mut v.widgets;
    for state in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        state.corner_radius = CornerRadius::same(WIDGET_RADIUS);
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.card_stroke);
    w.inactive.weak_bg_fill = p.button;
    w.inactive.bg_fill = p.button;
    w.inactive.bg_stroke = Stroke::NONE;
    w.hovered.weak_bg_fill = p.button_hover;
    w.hovered.bg_fill = p.button_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.6));
    w.hovered.expansion = 0.0;
    w.active.weak_bg_fill = p.accent.gamma_multiply(0.35);
    w.active.bg_fill = p.accent.gamma_multiply(0.35);
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.expansion = 0.0;
    w.open.weak_bg_fill = p.button_hover;
}

/// A rounded panel with a subtle fill and outline, for grouping content.
pub fn card(ui: &Ui) -> Frame {
    let p = palette(ui.visuals().dark_mode);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.card_stroke))
        .corner_radius(CARD_RADIUS)
        .inner_margin(Margin::same(16))
}

/// The navigation sidebar, a shade apart from the content.
pub fn sidebar_frame(ui: &Ui) -> Frame {
    let p = palette(ui.visuals().dark_mode);
    Frame::new().fill(p.sidebar).inner_margin(Margin::symmetric(10, 14))
}

/// The content area next to the sidebar.
pub fn content_frame(ui: &Ui) -> Frame {
    let p = palette(ui.visuals().dark_mode);
    Frame::new().fill(p.panel).inner_margin(Margin::same(18))
}

/// A card title.
pub fn card_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(16.5).strong());
    ui.add_space(4.0);
}

/// A small rounded label with a tinted background, e.g. "In use".
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(9)
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| ui.label(RichText::new(text).size(12.0).strong().color(color)))
        .response
}

/// A fixed-width speaker pill, so transcript text lines up.
pub fn speaker_pill(ui: &mut Ui, speaker: Speaker, faded: bool) {
    let color = speaker_color(speaker);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(62.0, 20.0), Sense::hover());
    let alpha = if faded { 0.5 } else { 1.0 };
    ui.painter().rect_filled(rect, 10, color.gamma_multiply(0.16 * alpha));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        speaker.label(),
        FontId::proportional(12.0),
        color.gamma_multiply(alpha),
    );
}

/// A slim rounded level meter; `level` is 0..=1.
pub fn level_meter(ui: &mut Ui, level: f32, color: Color32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 6.0), Sense::hover());
    let p = palette(ui.visuals().dark_mode);
    ui.painter().rect_filled(rect, 3, p.button);
    let level = level.clamp(0.0, 1.0);
    if level > 0.0 {
        let mut fill = rect;
        fill.set_width((rect.width() * level).max(rect.height()));
        ui.painter().rect_filled(fill, 3, color);
    }
}

/// The round record button: a red dot to start, a red square to stop.
pub fn record_button(ui: &mut Ui, recording: bool, enabled: bool) -> Response {
    let size = 46.0;
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let p = palette(ui.visuals().dark_mode);
    let center = rect.center();
    let hovered = enabled && response.hovered();
    let red = match (enabled, hovered) {
        (false, _) => ui.visuals().weak_text_color(),
        (true, true) => Color32::from_rgb(244, 82, 74),
        (true, false) => RECORD_RED,
    };
    let painter = ui.painter();
    painter.circle_filled(center, size / 2.0, if hovered { p.button_hover } else { p.button });
    painter.circle_stroke(center, size / 2.0 - 1.0, Stroke::new(1.5, red.gamma_multiply(0.5)));
    if recording {
        let square = egui::Rect::from_center_size(center, Vec2::splat(16.0));
        painter.rect_filled(square, 4, red);
    } else {
        painter.circle_filled(center, 11.0, red);
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// egui's built-in fonts have no Chinese, Japanese or Korean glyphs.
/// Adds system fonts that do, as fallbacks.
fn install_fallback_fonts(ctx: &egui::Context) {
    let linux = cfg!(not(any(target_os = "macos", windows)));
    let mut added = false;
    for (i, path) in fallback_font_paths().iter().enumerate() {
        if linux && added {
            break;
        }
        let Ok(bytes) = std::fs::read(path) else { continue };
        let data = egui::FontData::from_owned(bytes);
        let data = data.clone().tweak(baseline_tweak(&data));
        let families = [egui::FontFamily::Proportional, egui::FontFamily::Monospace]
            .into_iter()
            .map(|family| egui::epaint::text::InsertFontFamily {
                family,
                priority: egui::epaint::text::FontPriority::Lowest,
            })
            .collect();
        ctx.add_font(egui::epaint::text::FontInsert::new(
            &format!("fallback-{i}"),
            data,
            families,
        ));
        added = true;
    }
}

/// egui centres a fallback font's line height on egui's own font. CJK fonts
/// have much taller lines, so their characters sit a few pixels above the
/// baseline and lose their tops at the top edge of a scroll area. Measures
/// how far, and shifts that font's glyphs back down.
fn baseline_tweak(data: &egui::FontData) -> egui::epaint::text::FontTweak {
    const SIZE: f32 = 100.0;
    let mut definitions = egui::FontDefinitions::default();
    definitions
        .font_data
        .insert("fallback".into(), std::sync::Arc::new(data.clone()));
    definitions
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .push("fallback".into());
    let mut fonts = egui::epaint::text::Fonts::new(Default::default(), definitions);
    // A Latin letter, then Chinese, Japanese and Korean: whichever of those
    // the font has shows how far its glyphs are lifted.
    let galley =
        fonts
            .with_pixels_per_point(1.0)
            .layout_no_wrap("x中あ한".into(), FontId::proportional(SIZE), Color32::WHITE);
    let glyphs = &galley.rows[0].row.glyphs;
    let latin = glyphs[0].pos.y;
    let lift = glyphs[1..]
        .iter()
        .map(|g| latin - g.pos.y)
        .find(|lift| *lift != 0.0)
        .unwrap_or(0.0);
    egui::epaint::text::FontTweak {
        y_offset_factor: lift / SIZE,
        ..Default::default()
    }
}

#[cfg(target_os = "macos")]
fn fallback_font_paths() -> Vec<std::path::PathBuf> {
    [
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/Library/Fonts/Arial Unicode.ttf",
    ]
    .into_iter()
    .map(Into::into)
    .collect()
}

/// Microsoft YaHei, Yu Gothic and Malgun Gothic ship with Windows 10+, in
/// the Windows folder, which need not be on C:.
#[cfg(windows)]
fn fallback_font_paths() -> Vec<std::path::PathBuf> {
    let windows = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map_or_else(|| r"C:\Windows".into(), std::path::PathBuf::from);
    ["msyh.ttc", "YuGothM.ttc", "malgun.ttf"]
        .into_iter()
        .map(|name| windows.join("Fonts").join(name))
        .collect()
}

/// Noto CJK covers all three; its path differs between distributions, so
/// the usual places are tried first, then whatever fontconfig knows for
/// Chinese. One is enough, and each is large.
#[cfg(not(any(target_os = "macos", windows)))]
fn fallback_font_paths() -> Vec<std::path::PathBuf> {
    let mut paths: Vec<std::path::PathBuf> = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-sans-cjk-fonts/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    if !paths.iter().any(|p| p.is_file()) {
        paths.extend(fontconfig_cjk_fonts());
    }
    paths
}

/// Font files fontconfig lists as covering Chinese, if it is installed.
#[cfg(not(any(target_os = "macos", windows)))]
fn fontconfig_cjk_fonts() -> Vec<std::path::PathBuf> {
    let Ok(out) = std::process::Command::new("fc-list")
        .args([":lang=zh", "file"])
        .output()
    else {
        return Vec::new();
    };
    parse_fc_list(&String::from_utf8_lossy(&out.stdout))
}

/// `fc-list … file` prints `/path/to/font.ttc: ` per line.
#[cfg(any(not(any(target_os = "macos", windows)), test))]
fn parse_fc_list(output: &str) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<std::path::PathBuf> = output
        .lines()
        .map(|line| line.trim().trim_end_matches(':').trim())
        .filter(|p| {
            [".ttf", ".ttc", ".otf", ".otc"]
                .iter()
                .any(|ext| p.to_lowercase().ends_with(ext))
        })
        .map(Into::into)
        .collect();
    // fc-list's order is arbitrary; sorting keeps the choice stable.
    paths.sort();
    paths.dedup();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chinese next to Latin keeps its tops inside the line, so a scroll
    /// area's top edge does not cut them off. Needs a CJK system font.
    #[test]
    fn cjk_fallback_sits_on_the_baseline() {
        let Some(bytes) = fallback_font_paths().iter().find_map(|p| std::fs::read(p).ok()) else {
            return;
        };
        let data = egui::FontData::from_owned(bytes);
        let mut definitions = egui::FontDefinitions::default();
        let tweaked = data.clone().tweak(baseline_tweak(&data));
        definitions.font_data.insert("cjk".into(), std::sync::Arc::new(tweaked));
        definitions
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .push("cjk".into());
        let mut fonts = egui::epaint::text::Fonts::new(Default::default(), definitions);
        for pixels_per_point in [1.0, 2.0] {
            for size in [11.5, 14.5, 15.0, 21.0] {
                let galley = fonts.with_pixels_per_point(pixels_per_point).layout_no_wrap(
                    "x中".into(),
                    FontId::proportional(size),
                    Color32::WHITE,
                );
                let [latin, cjk] = [0, 1].map(|i| galley.rows[0].row.glyphs[i]);
                let top = cjk.pos.y + cjk.uv_rect.offset.y;
                let bottom = top + cjk.uv_rect.size.y;
                assert!(top >= -0.5, "top {top} at {size} pt, {pixels_per_point}x");
                // Chinese characters dip a little below the Latin baseline.
                assert!(
                    (bottom - latin.pos.y).abs() <= 0.25 * size,
                    "bottom {bottom} vs baseline {} at {size} pt",
                    latin.pos.y
                );
            }
        }
    }

    #[test]
    fn fc_list_output_is_parsed() {
        let out = "/usr/share/fonts/b/NotoSansCJK-VF.ttc: \n\
                   /usr/share/fonts/a/wqy-zenhei.ttc: \n\
                   /usr/share/fonts/c/font.pcf.gz: \n\
                   \n";
        assert_eq!(
            parse_fc_list(out),
            vec![
                std::path::PathBuf::from("/usr/share/fonts/a/wqy-zenhei.ttc"),
                std::path::PathBuf::from("/usr/share/fonts/b/NotoSansCJK-VF.ttc"),
            ]
        );
    }
}
