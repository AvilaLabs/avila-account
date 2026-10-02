//! The shared header pieces every Avila Labs egui tool shows at its top
//! right: the tool launcher (a square grid button that opens every tool)
//! and the account chip. Palette matches the ACTINV light theme.

use crate::{Manifest, Tool};
use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Id, Layout,
    Margin, Order, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
};

/// Re-exported so hosts need no second egui import path.
pub use egui;

pub const BLUE: Color32 = Color32::from_rgb(24, 0, 173);
pub const BLUE_BRIGHT: Color32 = Color32::from_rgb(42, 21, 214);
pub const BACKGROUND: Color32 = Color32::from_rgb(247, 248, 252);
pub const INK: Color32 = Color32::from_rgb(17, 17, 26);
pub const MUTED: Color32 = Color32::from_rgb(95, 105, 124);
pub const LINE: Color32 = Color32::from_rgb(220, 220, 226);
pub const TINT: Color32 = Color32::from_rgb(236, 234, 250);
/// Warning text, such as a failed sign-in.
pub const ALERT: Color32 = Color32::from_rgb(138, 66, 0);
/// Dark-theme accent for text on a dark card (the dashboard's cyan).
pub const ACCENT_DARK: Color32 = Color32::from_rgb(128, 220, 255);

/// The palette an account piece draws with, following the host's egui visuals.
#[derive(Clone, Copy)]
pub struct Pal {
    pub dark: bool,
    pub card: Color32,
    pub line: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub tint: Color32,
    /// Blue text, outlines and marks that must read on `card`.
    pub accent: Color32,
    /// Fill of primary buttons.
    pub primary: Color32,
}

pub fn pal(dark: bool) -> Pal {
    if dark {
        Pal {
            dark,
            card: Color32::from_rgb(28, 29, 38),
            line: Color32::from_rgb(72, 74, 96),
            ink: Color32::from_rgb(236, 237, 246),
            muted: Color32::from_rgb(160, 168, 186),
            tint: Color32::from_rgb(44, 44, 80),
            accent: ACCENT_DARK,
            primary: BLUE_BRIGHT,
        }
    } else {
        Pal {
            dark,
            card: Color32::WHITE,
            line: LINE,
            ink: INK,
            muted: MUTED,
            tint: TINT,
            accent: BLUE,
            primary: BLUE,
        }
    }
}

pub const LOGO: &[u8] = include_bytes!("../assets/avila-labs-logo.png");

/// What the viewer asked for. Hosts usually pass it to [`open`]; a desktop
/// tool may instead start a locally installed sibling.
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    Tool { id: String, url: String },
    Dashboard(String),
    SignIn(String),
}

/// Open the request in the browser (a new tab on the web build).
pub fn open(ctx: &egui::Context, launch: &Launch) {
    let url = match launch {
        Launch::Tool { url, .. } | Launch::Dashboard(url) | Launch::SignIn(url) => url,
    };
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

/// The tool marks, rendered from the Avila Labs tool SVGs at 96 px (2x of
/// the 48 px tile). A tool without one falls back to its monogram.
const TOOL_MARKS: [(&str, &[u8]); 3] = [
    ("actinv", include_bytes!("../assets/tools/actinv.png")),
    ("converra", include_bytes!("../assets/tools/converra.png")),
    ("openbnct", include_bytes!("../assets/tools/openbnct.png")),
];

/// The tool's mark as a texture, loaded once per egui context.
pub fn tool_mark(ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
    let (_, bytes) = TOOL_MARKS.iter().find(|(name, _)| *name == id)?;
    let key = Id::new(("avila-tool-mark", id));
    if let Some(texture) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(key)) {
        return Some(texture);
    }
    let decoded = image::load_from_memory(bytes).ok()?.into_rgba8();
    let texture = ctx.load_texture(
        format!("avila-tool-mark-{id}"),
        egui::ColorImage::from_rgba_unmultiplied(
            [decoded.width() as usize, decoded.height() as usize],
            decoded.as_raw(),
        ),
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| d.insert_temp(key, texture.clone()));
    Some(texture)
}

pub fn logo(ctx: &egui::Context) -> Result<egui::TextureHandle, image::ImageError> {
    let decoded = image::load_from_memory(LOGO)?.into_rgba8();
    Ok(ctx.load_texture(
        "avila-labs-logo",
        egui::ColorImage::from_rgba_unmultiplied(
            [decoded.width() as usize, decoded.height() as usize],
            decoded.as_raw(),
        ),
        egui::TextureOptions::LINEAR,
    ))
}

/// Signed-in state as the host knows it; a signed-in session carries the
/// display email.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Account {
    #[default]
    SignedOut,
    SignedIn {
        email: String,
    },
}

pub struct Launcher {
    pub manifest: Manifest,
    /// The tool this launcher is embedded in.
    pub current: String,
    /// Fields the viewer chose; their tools are listed first.
    pub fields: Vec<String>,
    pub open: bool,
}

impl Launcher {
    pub fn new(manifest: Manifest, current: impl Into<String>) -> Self {
        Self {
            manifest,
            current: current.into(),
            fields: Vec::new(),
            open: false,
        }
    }

    /// Draws the grid button and, when open, the tile panel beneath it.
    pub fn show(&mut self, ui: &mut Ui) -> Option<Launch> {
        let p = pal(ui.visuals().dark_mode);
        let (rect, response) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
        let response = response
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("Avila Labs tools");
        if response.clicked() {
            self.open = !self.open;
        }
        let active = self.open || response.hovered();
        if active {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, p.tint);
        }
        let dot = if active { p.accent } else { p.ink };
        for row in 0..3 {
            for col in 0..3 {
                let center =
                    rect.center() + vec2((col as f32 - 1.0) * 6.5, (row as f32 - 1.0) * 6.5);
                ui.painter().rect_filled(
                    Rect::from_center_size(center, vec2(3.5, 3.5)),
                    CornerRadius::ZERO,
                    dot,
                );
            }
        }
        if !self.open {
            return None;
        }

        let mut launched = None;
        let width = 344.0;
        // Kept on screen when the window is narrower than the panel.
        let left = (rect.right() - width).max(ui.ctx().content_rect().left() + 8.0);
        let pos = pos2(left, rect.bottom() + 8.0);
        let area = egui::Area::new(Id::new("avila-tool-launcher"))
            .order(Order::Foreground)
            .fade_in(false)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0, p.line))
                    .inner_margin(Margin::same(18))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 10],
                        blur: 28,
                        spread: 0,
                        color: Color32::from_black_alpha(28),
                    })
                    .show(ui, |ui| {
                        ui.set_width(width - 38.0);
                        launched = self.panel(ui);
                    });
            });
        // A click anywhere outside the panel or the button closes it.
        let clicked_outside = ui.ctx().input(|i| i.pointer.any_click())
            && !area.response.hovered()
            && !response.hovered();
        if launched.is_some()
            || clicked_outside
            || ui.ctx().input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.open = false;
        }
        launched
    }

    fn panel(&self, ui: &mut Ui) -> Option<Launch> {
        let p = pal(ui.visuals().dark_mode);
        let mut launched = None;
        ui.label(eyebrow("Avila Labs tools").color(p.accent));
        ui.add_space(10.0);
        let (mine, more) = self.manifest.arrange(&self.fields);
        if !self.fields.is_empty() {
            let names: Vec<&str> = self
                .fields
                .iter()
                .map(|f| self.manifest.field_name(f))
                .collect();
            ui.label(RichText::new(names.join(" · ")).size(12.0).color(p.muted));
            ui.add_space(6.0);
        }
        if let Some(l) = self.grid(ui, &mine) {
            launched = Some(l);
        }
        if !more.is_empty() {
            ui.add_space(12.0);
            ui.label(
                RichText::new("More from Avila Labs")
                    .size(12.0)
                    .color(p.muted),
            );
            ui.add_space(6.0);
            if let Some(l) = self.grid(ui, &more) {
                launched = Some(l);
            }
        }
        ui.add_space(14.0);
        let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(line, CornerRadius::ZERO, p.line);
        ui.add_space(10.0);
        let link = ui
            .horizontal(|ui| {
                let label = ui.add(
                    egui::Label::new(
                        RichText::new("All tools and your account")
                            .size(12.5)
                            .strong()
                            .color(p.accent),
                    )
                    .sense(Sense::click()),
                );
                // Drawn rather than typed: the default egui fonts have no arrow glyph.
                let (rect, arrow) = ui.allocate_exact_size(vec2(16.0, 12.0), Sense::click());
                let stroke = Stroke::new(1.4, p.accent);
                let (y, x0, x1) = (rect.center().y, rect.left() + 2.0, rect.right() - 2.0);
                ui.painter()
                    .line_segment([pos2(x0, y), pos2(x1, y)], stroke);
                ui.painter()
                    .line_segment([pos2(x1 - 4.0, y - 4.0), pos2(x1, y)], stroke);
                ui.painter()
                    .line_segment([pos2(x1 - 4.0, y + 4.0), pos2(x1, y)], stroke);
                label | arrow
            })
            .inner;
        if link.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            launched = Some(Launch::Dashboard(self.manifest.dashboard_url.clone()));
        }
        launched
    }

    fn grid(&self, ui: &mut Ui, tools: &[&Tool]) -> Option<Launch> {
        let mut launched = None;
        let tile = vec2(98.0, 94.0);
        for row in tools.chunks(3) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for tool in row {
                    if let Some(l) = self.tile(ui, tool, tile) {
                        launched = Some(l);
                    }
                }
            });
        }
        launched
    }

    fn tile(&self, ui: &mut Ui, tool: &Tool, size: egui::Vec2) -> Option<Launch> {
        let p = pal(ui.visuals().dark_mode);
        let current = tool.id == self.current;
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let response = response
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!(
                "{} — {}{}",
                tool.name,
                tool.summary,
                if current { " (this tool)" } else { "" }
            ));
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, CornerRadius::ZERO, p.tint);
        }
        let mark =
            Rect::from_center_size(pos2(rect.center().x, rect.top() + 30.0), vec2(40.0, 40.0));
        match tool_mark(painter.ctx(), &tool.id) {
            Some(texture) => {
                painter.image(
                    texture.id(),
                    mark,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                painter.rect_filled(
                    mark,
                    CornerRadius::ZERO,
                    if response.hovered() {
                        BLUE_BRIGHT
                    } else {
                        BLUE
                    },
                );
                painter.text(
                    mark.center(),
                    Align2::CENTER_CENTER,
                    &tool.monogram,
                    FontId::proportional(17.0),
                    Color32::WHITE,
                );
            }
        }
        painter.text(
            pos2(rect.center().x, rect.top() + 64.0),
            Align2::CENTER_CENTER,
            &tool.name,
            FontId::proportional(13.0),
            p.ink,
        );
        if current {
            painter.text(
                pos2(rect.center().x, rect.top() + 81.0),
                Align2::CENTER_CENTER,
                "OPEN",
                FontId::monospace(9.0),
                p.accent,
            );
        } else if tool.status != "released" {
            painter.text(
                pos2(rect.center().x, rect.top() + 81.0),
                Align2::CENTER_CENTER,
                tool.status.to_uppercase(),
                FontId::monospace(9.0),
                p.muted,
            );
        }
        if current {
            painter.rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(1.0, p.accent),
                StrokeKind::Inside,
            );
        }
        if response.clicked() && !current {
            return tool.open_url().map(|url| Launch::Tool {
                id: tool.id.clone(),
                url: url.to_owned(),
            });
        }
        None
    }
}

/// The account chip: "Sign in" when signed out, a blue initial when in.
pub fn account_chip(ui: &mut Ui, manifest: &Manifest, account: &Account) -> Option<Launch> {
    let p = pal(ui.visuals().dark_mode);
    match account {
        Account::SignedOut => {
            let button =
                egui::Button::new(RichText::new("Sign in").size(12.5).strong().color(p.accent))
                    .fill(p.card)
                    .stroke(Stroke::new(1.0, p.accent))
                    .corner_radius(CornerRadius::ZERO)
                    .min_size(vec2(72.0, 30.0));
            ui.add(button)
                .on_hover_text("Optional. Every tool works fully without an account.")
                .clicked()
                .then(|| Launch::SignIn(manifest.account_url.clone()))
        }
        Account::SignedIn { email } => {
            let initial = email
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            let (rect, response) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, p.primary);
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                initial,
                FontId::proportional(14.0),
                Color32::WHITE,
            );
            response
                .on_hover_text(format!("Signed in as {email}"))
                .clicked()
                .then(|| Launch::Dashboard(manifest.dashboard_url.clone()))
        }
    }
}

/// Launcher then account chip, laid out right to left. Call inside the
/// host's header row.
pub fn header_right(ui: &mut Ui, launcher: &mut Launcher, account: &Account) -> Option<Launch> {
    let mut launched = None;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if let Some(l) = account_chip(ui, &launcher.manifest, account) {
            launched = Some(l);
        }
        if let Some(l) = launcher.show(ui) {
            launched = Some(l);
        }
    });
    launched
}

pub fn eyebrow(text: &str) -> RichText {
    RichText::new(text.to_uppercase())
        .monospace()
        .size(10.0)
        .extra_letter_spacing(1.6)
        .color(BLUE)
}
