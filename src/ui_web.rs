//! The browser build's account pieces: [`WebSuite`] draws the header-right
//! controls (launcher, then "Sign in" or the account chip) and the
//! first-visit sign-in prompt. Feature `ui`.
//!
//! The session lives in an HttpOnly cookie on the account service, so the
//! page only asks `GET {base}/api/auth/me` (credentials included) on start
//! and whenever the tab becomes visible again. Any failure, including a
//! blocked cross-origin call, reads as signed out and shows no error: every
//! tool works without an account.
//!
//! The URL, response and prompt rules are plain functions that build and
//! test on every target; only the browser plumbing is wasm32-only.

use crate::ui::{self, pal, Launch, Launcher};
use crate::{Manifest, DEFAULT_MANIFEST};
use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Id, Layout,
    Margin, Order, RichText, Sense, Stroke, Ui,
};

/// The account service the web build talks to unless `AVILA_ACCOUNT_BASE`
/// was set when this crate was compiled.
pub const DEFAULT_BASE: &str = "https://api.avilalabs.org";

/// localStorage key set once the viewer dismisses the prompt.
pub const DISMISS_KEY: &str = "avila.signin.prompt.dismissed";

pub const PROMPT_TITLE: &str = "Sign in to Avila Labs";
pub const PROMPT_BODY: &str =
    "One account for ACTINV, Converra and OpenBNCT. Every tool works without an account.";

/// What the page knows about the viewer.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum WebState {
    /// No answer yet: nothing is drawn, so the header does not flash.
    #[default]
    Unknown,
    SignedOut,
    SignedIn {
        email: String,
    },
}

/// The service base URL, without a trailing slash.
pub fn account_base() -> String {
    base_or_default(option_env!("AVILA_ACCOUNT_BASE"))
}

fn base_or_default(configured: Option<&str>) -> String {
    configured
        .map(|b| b.trim().trim_end_matches('/'))
        .filter(|b| !b.is_empty())
        .unwrap_or(DEFAULT_BASE)
        .to_owned()
}

/// Percent-encode everything but RFC 3986 unreserved characters, so a whole
/// URL survives as one query value.
pub fn encode_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `{base}/{page}?next=<return url>` for the service's sign-in and
/// registration pages.
pub fn page_url(base: &str, page: &str, next: &str) -> String {
    format!(
        "{}/{page}?next={}",
        base.trim_end_matches('/'),
        encode_component(next)
    )
}

/// Read a `GET /api/auth/me` reply: only a 200 with `user.email` is signed in.
pub fn parse_me(status: u16, body: &str) -> WebState {
    if status != 200 {
        return WebState::SignedOut;
    }
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["user"]["email"].as_str().map(str::to_owned))
        .filter(|email| !email.is_empty())
        .map_or(WebState::SignedOut, |email| WebState::SignedIn { email })
}

/// The prompt shows to a signed-out viewer who has not dismissed it.
pub fn prompt_visible(state: &WebState, dismissed: bool) -> bool {
    *state == WebState::SignedOut && !dismissed
}

fn initial(email: &str) -> String {
    email
        .chars()
        .next()
        .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().to_string())
}

/// Launcher, account state and prompt for one browser tool.
pub struct WebSuite {
    pub launcher: Launcher,
    base: String,
    menu_open: bool,
    #[cfg(target_arch = "wasm32")]
    browser: browser::Shared,
    #[cfg(not(target_arch = "wasm32"))]
    state: WebState,
    #[cfg(not(target_arch = "wasm32"))]
    dismissed: bool,
}

#[cfg(target_arch = "wasm32")]
impl WebSuite {
    /// `current` is this tool's id in the manifest (`actinv`, ...). Starts
    /// the first account check and re-checks when the tab becomes visible.
    pub fn new(ctx: &egui::Context, current: &str) -> Self {
        let base = account_base();
        let browser = browser::Shared::start(ctx, &base);
        Self {
            launcher: Launcher::new(manifest(), current),
            base,
            menu_open: false,
            browser,
        }
    }

    pub fn state(&self) -> WebState {
        self.browser.state()
    }

    fn dismissed(&self) -> bool {
        self.browser.dismissed()
    }

    fn dismiss(&self) {
        self.browser.dismiss();
    }

    fn navigate(&self, url: &str) {
        browser::navigate(url);
    }

    fn sign_out(&self, ctx: &egui::Context) {
        self.browser.sign_out(ctx);
    }

    fn here(&self) -> String {
        browser::current_url()
    }
}

/// Native stand-in so the layout code is one body; the native tools keep
/// their device sign-in and never construct this.
#[cfg(not(target_arch = "wasm32"))]
impl WebSuite {
    pub fn new(_ctx: &egui::Context, current: &str) -> Self {
        Self {
            launcher: Launcher::new(manifest(), current),
            base: account_base(),
            menu_open: false,
            state: WebState::SignedOut,
            dismissed: false,
        }
    }

    pub fn state(&self) -> WebState {
        self.state.clone()
    }

    fn dismissed(&self) -> bool {
        self.dismissed
    }

    fn dismiss(&mut self) {
        self.dismissed = true;
    }

    fn navigate(&self, _url: &str) {}

    fn sign_out(&mut self, _ctx: &egui::Context) {
        self.state = WebState::SignedOut;
    }

    fn here(&self) -> String {
        String::new()
    }
}

pub(crate) fn manifest() -> Manifest {
    Manifest::parse(DEFAULT_MANIFEST).expect("the embedded manifest is valid")
}

impl WebSuite {
    /// The launcher, then "Sign in" or the account chip, right to left.
    /// Call inside the host's header row; the controls sit at its far right.
    pub fn header_right(&mut self, ui: &mut Ui) {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.controls(ui));
    }

    /// The same controls for a ui that is already right to left, such as
    /// the right side of [`egui::containers::Sides`].
    pub fn controls(&mut self, ui: &mut Ui) {
        let mut launched = None;
        let state = self.state();
        ui.spacing_mut().item_spacing.x = 10.0;
        match &state {
            WebState::Unknown => {}
            WebState::SignedOut => self.sign_in_button(ui),
            WebState::SignedIn { email } => launched = self.chip(ui, email),
        }
        if let Some(l) = self.launcher.show(ui) {
            launched = Some(l);
        }
        if let Some(launch) = launched {
            ui::open(ui.ctx(), &launch);
        }
    }

    fn sign_in_button(&mut self, ui: &mut Ui) {
        if sign_in_button(ui) {
            self.navigate(&page_url(&self.base, "signin", &self.here()));
        }
    }

    /// The round initial and its menu. Returns the dashboard launch when
    /// chosen; sign out is handled here.
    fn chip(&mut self, ui: &mut Ui, email: &str) -> Option<Launch> {
        let dashboard = format!("{}/suite", self.base);
        match account_chip(ui, email, &mut self.menu_open) {
            ChipAction::None => None,
            ChipAction::Dashboard => Some(Launch::Dashboard(dashboard)),
            ChipAction::SignOut => {
                self.sign_out(ui.ctx());
                None
            }
        }
    }

    /// The first-visit card. `top` is the y just under the host's header.
    /// Call after the header is laid out; it floats over the page.
    pub fn prompt(&mut self, ctx: &egui::Context, top: f32) {
        if !prompt_visible(&self.state(), self.dismissed()) {
            return;
        }
        match first_visit_card(ctx, top) {
            PromptAction::None => {}
            PromptAction::Create => self.navigate(&page_url(&self.base, "register", &self.here())),
            PromptAction::SignIn => self.navigate(&page_url(&self.base, "signin", &self.here())),
            PromptAction::Dismiss => self.dismiss(),
        }
    }
}

/// "Sign in": outlined, blue text, as tall as the launcher button.
pub(crate) fn sign_in_button(ui: &mut Ui) -> bool {
    let p = pal(ui.visuals().dark_mode);
    let button = egui::Button::new(RichText::new("Sign in").size(12.5).strong().color(p.accent))
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.accent))
        .corner_radius(CornerRadius::ZERO)
        .min_size(vec2(72.0, 32.0));
    ui.add(button)
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Optional. Every tool works fully without an account.")
        .clicked()
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChipAction {
    None,
    Dashboard,
    SignOut,
}

/// The round initial chip and its menu (email, "Open dashboard", "Sign out").
pub(crate) fn account_chip(ui: &mut Ui, email: &str, menu_open: &mut bool) -> ChipAction {
    let p = pal(ui.visuals().dark_mode);
    let (rect, response) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let response = response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(format!("Signed in as {email}"));
    if response.clicked() {
        *menu_open = !*menu_open;
    }
    ui.painter().circle_filled(rect.center(), 16.0, p.primary);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        initial(email),
        FontId::proportional(14.0),
        Color32::WHITE,
    );
    if !*menu_open {
        return ChipAction::None;
    }
    let mut action = ChipAction::None;
    let width = 220.0;
    let screen = ui.ctx().content_rect();
    let x = (rect.right() - width).max(screen.left() + 8.0);
    let area = egui::Area::new(Id::new("avila-web-account-menu"))
        .order(Order::Foreground)
        .fade_in(false)
        .fixed_pos(pos2(x, rect.bottom() + 8.0))
        .show(ui.ctx(), |ui| {
            card(ui, width, 14, |ui| {
                ui.label(RichText::new(email).size(12.0).color(p.muted));
                ui.add_space(8.0);
                if menu_item(ui, "Open dashboard") {
                    action = ChipAction::Dashboard;
                }
                if menu_item(ui, "Sign out") {
                    action = ChipAction::SignOut;
                }
            });
        });
    let clicked_outside = ui.ctx().input(|i| i.pointer.any_click())
        && !area.response.hovered()
        && !response.hovered();
    if action != ChipAction::None
        || clicked_outside
        || ui.ctx().input(|i| i.key_pressed(egui::Key::Escape))
    {
        *menu_open = false;
    }
    action
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PromptAction {
    None,
    Create,
    SignIn,
    Dismiss,
}

/// The first-visit card, anchored top-right under the header (`top` is the
/// y just below it). Drawing only; the host decides when to show it.
pub(crate) fn first_visit_card(ctx: &egui::Context, top: f32) -> PromptAction {
    let p = pal(ctx.global_style().visuals.dark_mode);
    let screen = ctx.content_rect();
    let width = 340.0_f32.min(screen.width() - 16.0).max(200.0);
    let mut action = PromptAction::None;
    egui::Area::new(Id::new("avila-web-signin-prompt"))
        .order(Order::Foreground)
        .fade_in(false)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(pos2(screen.right() - 8.0, top + 8.0))
        .show(ctx, |ui| {
            card(ui, width, 18, |ui| {
                ui.label(ui::eyebrow("Avila Labs").color(p.accent));
                ui.add_space(6.0);
                ui.label(RichText::new(PROMPT_TITLE).size(16.0).strong().color(p.ink));
                ui.add_space(6.0);
                ui.label(RichText::new(PROMPT_BODY).size(12.5).color(p.muted));
                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    let primary = egui::Button::new(
                        RichText::new("Create account")
                            .size(12.5)
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(p.primary)
                    .corner_radius(CornerRadius::ZERO)
                    .min_size(vec2(0.0, 30.0));
                    if ui.add(primary).clicked() {
                        action = PromptAction::Create;
                    }
                    let secondary = egui::Button::new(
                        RichText::new("Sign in").size(12.5).strong().color(p.accent),
                    )
                    .fill(p.card)
                    .stroke(Stroke::new(1.0, p.accent))
                    .corner_radius(CornerRadius::ZERO)
                    .min_size(vec2(0.0, 30.0));
                    if ui.add(secondary).clicked() {
                        action = PromptAction::SignIn;
                    }
                    let later =
                        egui::Button::new(RichText::new("Not now").size(12.5).color(p.muted))
                            .frame(false)
                            .min_size(vec2(0.0, 30.0));
                    if ui
                        .add(later)
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        action = PromptAction::Dismiss;
                    }
                });
            });
        });
    action
}

/// A white card with the launcher's border and shadow.
fn card(ui: &mut Ui, width: f32, margin: i8, add: impl FnOnce(&mut Ui)) {
    let p = pal(ui.visuals().dark_mode);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.line))
        .inner_margin(Margin::same(margin))
        .shadow(egui::epaint::Shadow {
            offset: [0, 10],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(28),
        })
        .show(ui, |ui| {
            ui.set_width(width - 2.0 * f32::from(margin) - 2.0);
            add(ui);
        });
}

fn menu_item(ui: &mut Ui, label: &str) -> bool {
    let p = pal(ui.visuals().dark_mode);
    let item = egui::Button::new(RichText::new(label).size(13.0).color(p.ink))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::ZERO)
        .min_size(vec2(ui.available_width(), 30.0));
    let response = ui.add(item).on_hover_cursor(CursorIcon::PointingHand);
    if response.hovered() {
        ui.painter()
            .rect_filled(response.rect, CornerRadius::ZERO, p.tint);
    }
    response.clicked()
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::{parse_me, WebState, DISMISS_KEY};
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::{spawn_local, JsFuture};
    use web_sys::{Headers, Request, RequestCredentials, RequestInit, RequestMode, Response};

    struct Inner {
        state: WebState,
        dismissed: bool,
        base: String,
    }

    #[derive(Clone)]
    pub struct Shared(Rc<RefCell<Inner>>);

    impl Shared {
        pub fn start(ctx: &egui::Context, base: &str) -> Self {
            let dismissed = storage()
                .and_then(|s| s.get_item(DISMISS_KEY).ok().flatten())
                .is_some_and(|v| v == "1");
            let shared = Shared(Rc::new(RefCell::new(Inner {
                state: WebState::Unknown,
                dismissed,
                base: base.to_owned(),
            })));
            shared.refresh(ctx);
            if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                let (again, ctx) = (shared.clone(), ctx.clone());
                let on_visible = Closure::<dyn FnMut()>::new({
                    let document = document.clone();
                    move || {
                        if !document.hidden() {
                            again.refresh(&ctx);
                        }
                    }
                });
                let _ = document.add_event_listener_with_callback(
                    "visibilitychange",
                    on_visible.as_ref().unchecked_ref(),
                );
                on_visible.forget();
            }
            shared
        }

        pub fn state(&self) -> WebState {
            self.0.borrow().state.clone()
        }

        pub fn dismissed(&self) -> bool {
            self.0.borrow().dismissed
        }

        pub fn dismiss(&self) {
            self.0.borrow_mut().dismissed = true;
            if let Some(storage) = storage() {
                let _ = storage.set_item(DISMISS_KEY, "1");
            }
        }

        fn refresh(&self, ctx: &egui::Context) {
            let (this, ctx) = (self.clone(), ctx.clone());
            let url = format!("{}/api/auth/me", self.0.borrow().base);
            spawn_local(async move {
                let state = match request(&url, "GET", false).await {
                    Some((status, body)) => parse_me(status, &body),
                    None => WebState::SignedOut,
                };
                this.0.borrow_mut().state = state;
                ctx.request_repaint();
            });
        }

        pub fn sign_out(&self, ctx: &egui::Context) {
            let (this, ctx) = (self.clone(), ctx.clone());
            let url = format!("{}/api/auth/logout", self.0.borrow().base);
            spawn_local(async move {
                let _ = request(&url, "POST", true).await;
                this.refresh(&ctx);
            });
        }
    }

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    pub fn current_url() -> String {
        web_sys::window()
            .and_then(|w| w.location().href().ok())
            .unwrap_or_default()
    }

    /// Same-tab navigation.
    pub fn navigate(url: &str) {
        if let Some(window) = web_sys::window() {
            let _ = window.location().assign(url);
        }
    }

    /// One credentialed request; `None` on any network or CORS failure.
    async fn request(url: &str, method: &str, csrf: bool) -> Option<(u16, String)> {
        let init = RequestInit::new();
        init.set_method(method);
        init.set_mode(RequestMode::Cors);
        init.set_credentials(RequestCredentials::Include);
        if csrf {
            let headers = Headers::new().ok()?;
            headers.set("X-Avila-CSRF", "1").ok()?;
            init.set_headers(&headers);
        }
        let request = Request::new_with_str_and_init(url, &init).ok()?;
        let response = JsFuture::from(web_sys::window()?.fetch_with_request(&request))
            .await
            .ok()?;
        let response: Response = response.dyn_into().ok()?;
        let text = JsFuture::from(response.text().ok()?).await.ok()?;
        Some((response.status(), text.as_string().unwrap_or_default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_defaults_trims_and_overrides() {
        assert_eq!(base_or_default(None), "https://api.avilalabs.org");
        assert_eq!(base_or_default(Some("  ")), DEFAULT_BASE);
        assert_eq!(
            base_or_default(Some("http://127.0.0.1:8820/")),
            "http://127.0.0.1:8820"
        );
    }

    #[test]
    fn next_is_one_encoded_query_value() {
        let url = page_url(
            "http://127.0.0.1:8820/",
            "signin",
            "https://actinv.avilalabs.org/app/?a=1&b=x y#top",
        );
        assert_eq!(
            url,
            "http://127.0.0.1:8820/signin?next=https%3A%2F%2Factinv.avilalabs.org%2Fapp%2F%3Fa%3D1%26b%3Dx%20y%23top"
        );
        assert!(page_url(DEFAULT_BASE, "register", "https://x.org/é")
            .ends_with("https%3A%2F%2Fx.org%2F%C3%A9"));
    }

    #[test]
    fn me_reply_is_signed_in_only_with_an_email() {
        let ok = r#"{"user":{"email":"a@b.org"},"owner":"user:1"}"#;
        assert_eq!(
            parse_me(200, ok),
            WebState::SignedIn {
                email: "a@b.org".into()
            }
        );
        assert_eq!(parse_me(401, ok), WebState::SignedOut);
        assert_eq!(parse_me(200, r#"{"error":"x"}"#), WebState::SignedOut);
        assert_eq!(
            parse_me(200, r#"{"user":{"email":""}}"#),
            WebState::SignedOut
        );
        assert_eq!(parse_me(200, "not json"), WebState::SignedOut);
    }

    #[test]
    fn prompt_shows_only_when_signed_out_and_not_dismissed() {
        let email = WebState::SignedIn {
            email: "a@b.org".into(),
        };
        assert!(prompt_visible(&WebState::SignedOut, false));
        assert!(!prompt_visible(&WebState::SignedOut, true));
        assert!(!prompt_visible(&WebState::Unknown, false));
        assert!(!prompt_visible(&email, false));
    }

    #[test]
    fn initial_is_uppercase_first_letter() {
        assert_eq!(initial("ada@x.org"), "A");
        assert_eq!(initial(""), "?");
    }

    #[test]
    fn embedded_manifest_points_sign_in_at_signin() {
        assert!(manifest().account_url.ends_with("/signin"));
    }
}
