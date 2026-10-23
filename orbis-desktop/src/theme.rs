use eframe::egui::{self, Color32, FontFamily, FontId, RichText, Stroke};

pub const ACCENT: Color32 = Color32::from_rgb(28, 100, 211);

pub fn configure(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let regular = egui::FontData::from_static(include_bytes!("../assets/fonts/Inter.ttf"));
    let medium = regular.clone().tweak(egui::FontTweak {
        coords: egui::epaint::text::VariationCoords::new([(b"wght", 600.0)]),
        ..Default::default()
    });
    fonts.font_data.insert("Inter".into(), regular.into());
    fonts.font_data.insert("Inter Medium".into(), medium.into());
    let fallback = fonts.families.get_mut(&FontFamily::Proportional).unwrap();
    fallback.insert(0, "Inter".into());
    let mut heading = fallback.clone();
    heading.insert(0, "Inter Medium".into());
    fonts
        .families
        .insert(FontFamily::Name("heading".into()), heading);
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.indent = 16.0;
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(12.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, heading_font(18.0));
        let dark = style.visuals.dark_mode;
        let text = if dark {
            Color32::from_rgb(232, 236, 241)
        } else {
            Color32::from_rgb(29, 38, 53)
        };
        let muted = if dark {
            Color32::from_rgb(166, 177, 192)
        } else {
            Color32::from_rgb(91, 104, 123)
        };
        let border = if dark {
            Color32::from_rgb(61, 71, 87)
        } else {
            Color32::from_rgb(214, 222, 232)
        };
        let surface = if dark {
            Color32::from_rgb(25, 31, 40)
        } else {
            Color32::from_rgb(248, 250, 253)
        };
        style.visuals.panel_fill = surface;
        style.visuals.window_fill = surface;
        style.visuals.extreme_bg_color = if dark {
            Color32::from_rgb(17, 23, 31)
        } else {
            Color32::WHITE
        };
        style.visuals.faint_bg_color = if dark {
            Color32::from_rgb(34, 43, 56)
        } else {
            Color32::from_rgb(234, 240, 248)
        };
        style.visuals.override_text_color = Some(text);
        style.visuals.weak_text_color = Some(muted);
        style.visuals.selection.bg_fill = if dark {
            Color32::from_rgb(31, 65, 106)
        } else {
            Color32::from_rgb(221, 234, 254)
        };
        style.visuals.selection.stroke = Stroke::new(
            1.0,
            if dark {
                Color32::from_rgb(156, 198, 255)
            } else {
                ACCENT
            },
        );
        style.visuals.hyperlink_color = if dark {
            Color32::from_rgb(156, 198, 255)
        } else {
            ACCENT
        };
        for widget in [
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            widget.corner_radius = egui::CornerRadius::same(6);
            widget.bg_stroke = Stroke::new(1.0, border);
            widget.fg_stroke = Stroke::new(1.0, text);
            widget.expansion = 0.0;
        }
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text);
        style.visuals.widgets.inactive.weak_bg_fill = if dark {
            Color32::from_rgb(35, 43, 55)
        } else {
            Color32::WHITE
        };
        style.visuals.widgets.inactive.bg_fill = style.visuals.widgets.inactive.weak_bg_fill;
        style.visuals.widgets.hovered.weak_bg_fill = if dark {
            Color32::from_rgb(45, 58, 76)
        } else {
            Color32::from_rgb(235, 241, 250)
        };
        style.visuals.widgets.active.weak_bg_fill = style.visuals.selection.bg_fill;
    });
}

fn heading_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("heading".into()))
}
pub fn heading(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(heading_font(size))
}
pub fn muted(ui: &egui::Ui, text: impl Into<String>) -> RichText {
    RichText::new(text).color(ui.visuals().weak_text_color())
}
pub fn primary(text: &str) -> egui::Button<'_> {
    egui::Button::new(heading(text, 14.0).color(Color32::WHITE))
        .fill(ACCENT)
        .stroke(Stroke::NONE)
        .min_size(egui::vec2(132.0, 36.0))
}
pub fn attention(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(245, 190, 112)
    } else {
        Color32::from_rgb(145, 78, 13)
    }
}

/// The existing Orbis package mark, drawn as a small vector so it remains crisp.
pub fn package_mark(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let dark = ui.visuals().dark_mode;
    let fill = if dark {
        Color32::from_rgb(31, 53, 79)
    } else {
        Color32::from_rgb(226, 237, 252)
    };
    let ink = if dark {
        Color32::from_rgb(135, 185, 244)
    } else {
        ACCENT
    };
    let p = |x: f32, y: f32| rect.min + egui::vec2(x * size, y * size);
    ui.painter().rect_filled(rect, (size * 0.2) as u8, fill);
    let stroke = Stroke::new((size * 0.025).max(1.4), ink);
    ui.painter().add(egui::Shape::closed_line(
        vec![
            p(0.5, 0.22),
            p(0.77, 0.36),
            p(0.77, 0.65),
            p(0.5, 0.8),
            p(0.23, 0.65),
            p(0.23, 0.36),
        ],
        stroke,
    ));
    for line in [
        [p(0.23, 0.36), p(0.5, 0.51)],
        [p(0.5, 0.51), p(0.77, 0.36)],
        [p(0.5, 0.51), p(0.5, 0.8)],
        [p(0.37, 0.29), p(0.64, 0.44)],
        [p(0.64, 0.44), p(0.64, 0.59)],
    ] {
        ui.painter().line_segment(line, stroke);
    }
}
