//! "Midnight Teal" theme for the InputSync GUI.
//!
//! A cohesive dark color system (GitHub-dark-inspired background tones + a
//! teal accent). All UI code should reference these constants instead of
//! hardcoding RGB values, so the palette stays consistent and tunable in
//! one place.
//!
//! Some helpers (card_hover, status_dot, mono) are not yet used by every
//! panel but are part of the theme API for future use.

#![allow(dead_code)]

use egui::{Color32, Rounding, Stroke, Vec2};

// ---- Color palette --------------------------------------------------------

/// Deep charcoal/near-black — the window background.
pub const BG_DARK: Color32 = Color32::from_rgb(13, 17, 23);
/// Slightly lifted surface — cards and framed groups.
pub const BG_CARD: Color32 = Color32::from_rgb(22, 27, 34);
/// Inset background — input fields, code blocks, secondary surfaces.
pub const BG_INSET: Color32 = Color32::from_rgb(11, 15, 21);
/// Subtle separator / card border.
pub const BORDER: Color32 = Color32::from_rgb(48, 54, 61);

/// Primary text (headings, values).
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(230, 237, 243);
/// Secondary text (labels, hints, dimmed info).
pub const TEXT_DIM: Color32 = Color32::from_rgb(139, 148, 158);

/// Teal — the accent / primary-action color.
pub const ACCENT: Color32 = Color32::from_rgb(45, 212, 191);
/// Darker teal — button hover / pressed.
pub const ACCENT_DIM: Color32 = Color32::from_rgb(20, 184, 166);

/// Green — connected / scanning / success.
pub const SUCCESS: Color32 = Color32::from_rgb(74, 222, 128);
/// Amber — idle / caution.
pub const WARNING: Color32 = Color32::from_rgb(251, 191, 36);
/// Red — error / destructive action (Stop, Disconnect).
pub const DANGER: Color32 = Color32::from_rgb(248, 113, 113);

// ---- Visuals builder ------------------------------------------------------

/// Build a fully-configured `egui::Visuals` from the Midnight Teal palette.
/// Call once at startup via `ctx.set_visuals(theme::visuals())`.
pub fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();

    // Panel / window fills.
    v.panel_fill = BG_DARK;
    v.window_fill = BG_CARD;
    v.extreme_bg_color = BG_INSET; // input fields, combo boxes
    v.faint_bg_color = BG_CARD; // alternating rows, faint surfaces

    // Hyperlinks / selection use the accent.
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.linear_multiply(0.30);
    v.selection.stroke = Stroke::new(1.0, ACCENT);

    // Widget styling: rounded corners, accent highlights, card-tone fills.
    let w = &mut v.widgets;
    w.inactive.bg_fill = BG_INSET;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.rounding = Rounding::same(6.0);

    w.hovered.bg_fill = Color32::from_rgb(28, 35, 48);
    w.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    w.hovered.rounding = Rounding::same(6.0);

    w.active.bg_fill = ACCENT_DIM;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.rounding = Rounding::same(6.0);

    w.noninteractive.bg_fill = BG_DARK;
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.rounding = Rounding::same(8.0);

    // Open buttons / windows get a subtle teal-tinted stroke.
    v.window_stroke = Stroke::new(1.0, BORDER);

    v
}

/// Tune the global `Style`: larger default font, comfortable spacing,
/// rounded buttons. Call after `set_visuals`.
pub fn style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    // Slightly larger text for readability.
    for tf in style.text_styles.values_mut() {
        tf.size *= 1.05;
    }
    // Bump the heading size specifically.
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(24.0));
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(14.0, 8.0);
    style.spacing.window_margin = egui::Margin::same(16.0);
    ctx.set_style(style);
}

// ---- UI helpers -----------------------------------------------------------

/// Render children inside a rounded card frame (card background + border).
///
/// ```ignore
/// theme::card(ui, |ui| {
///     ui.label("inside a card");
/// });
/// ```
pub fn card<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::group(ui.style())
        .fill(BG_CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(10.0))
        .inner_margin(egui::Margin {
            left: 16.0,
            right: 16.0,
            top: 14.0,
            bottom: 14.0,
        })
        .show(ui, add_contents)
        .inner
}

/// A card whose background lifts on hover (for selectable / clickable cards).
pub fn card_hover<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let response = ui.interact(
        ui.max_rect(),
        ui.next_auto_id().with("card_hover"),
        egui::Sense::hover(),
    );
    let fill = if response.hovered() {
        Color32::from_rgb(28, 35, 48)
    } else {
        BG_CARD
    };
    let stroke = if response.hovered() {
        Stroke::new(1.5, ACCENT)
    } else {
        Stroke::new(1.0, BORDER)
    };
    egui::Frame::group(ui.style())
        .fill(fill)
        .stroke(stroke)
        .rounding(Rounding::same(12.0))
        .inner_margin(egui::Margin::same(16.0))
        .show(ui, add_contents)
        .inner
}

/// A primary accent button: teal fill, dark text, large rounded. Returns true
/// if clicked. Use for the main action (Run, Connect).
pub fn accent_button(ui: &mut egui::Ui, text: impl Into<String>) -> bool {
    accent_button_sized(ui, text, Vec2::new(130.0, 38.0))
}

pub fn accent_button_sized(ui: &mut egui::Ui, text: impl Into<String>, min_size: Vec2) -> bool {
    let btn = egui::Button::new(egui::RichText::new(text).color(BG_DARK).strong())
        .min_size(min_size)
        .fill(ACCENT)
        .stroke(Stroke::new(1.0, ACCENT_DIM))
        .rounding(Rounding::same(8.0));
    ui.add(btn).clicked()
}

/// A danger button: red fill, white text. For Stop / Disconnect.
pub fn danger_button(ui: &mut egui::Ui, text: impl Into<String>) -> bool {
    let btn = egui::Button::new(egui::RichText::new(text).color(Color32::WHITE).strong())
        .min_size(Vec2::new(130.0, 38.0))
        .fill(DANGER)
        .stroke(Stroke::new(1.0, DANGER.linear_multiply(0.7)))
        .rounding(Rounding::same(8.0));
    ui.add(btn).clicked()
}

/// A ghost / outline button: transparent bg, border, dim text. For secondary
/// actions (Change role, Retry).
pub fn ghost_button(ui: &mut egui::Ui, text: impl Into<String>) -> bool {
    let btn = egui::Button::new(egui::RichText::new(text).color(TEXT_DIM))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(6.0));
    ui.add(btn).clicked()
}

/// Paint a small colored dot (status indicator). Pass SUCCESS / WARNING / DANGER.
pub fn status_dot(ui: &mut egui::Ui, color: Color32, label: &str) {
    ui.horizontal(|ui| {
        let (response, painter) = ui.allocate_painter(Vec2::new(10.0, 10.0), egui::Sense::hover());
        let center = response.rect.center();
        painter.circle_filled(center, 4.0, color);
        // Subtle glow ring.
        painter.circle_stroke(center, 5.5, Stroke::new(1.0, color.linear_multiply(0.4)));
        ui.label(egui::RichText::new(label).color(color).strong());
    });
}

/// A "pill" label: colored text on a faint colored background.
pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    let galley =
        ui.fonts(|f| f.layout_no_wrap(text.to_string(), egui::FontId::proportional(13.0), color));
    let size = galley.size();
    let pad = 10.0;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(size.x + pad * 2.0, size.y + 6.0),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, Rounding::same(12.0), color.linear_multiply(0.15));
    ui.painter().galley(
        egui::pos2(rect.left() + pad, rect.top() + 3.0),
        galley,
        egui::Color32::TRANSPARENT,
    );
    // Re-position cursor after the drawn pill so subsequent widgets flow.
    ui.advance_cursor_after_rect(rect);
}

/// Heading text in the primary color.
pub fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).heading().color(TEXT_PRIMARY));
}

/// A dim secondary label.
pub fn dim_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(TEXT_DIM));
}

/// Monospace text on the inset background (for fingerprints, addresses).
pub fn mono(ui: &mut egui::Ui, text: &str) {
    let galley = ui.fonts(|f| {
        f.layout(
            text.to_string(),
            egui::FontId::monospace(13.0),
            TEXT_PRIMARY,
            f32::MAX,
        )
    });
    let pad = 8.0;
    let (rect, _) = ui.allocate_exact_size(
        galley.size() + Vec2::new(pad * 2.0, 4.0),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, Rounding::same(5.0), BG_INSET);
    ui.painter().galley(
        egui::pos2(rect.left() + pad, rect.top() + 2.0),
        galley,
        egui::Color32::TRANSPARENT,
    );
    ui.advance_cursor_after_rect(rect);
}
