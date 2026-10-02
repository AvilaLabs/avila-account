//! The desktop build's account pieces: [`DesktopSuite`] draws the same
//! header-right controls and first-visit prompt as the browser build
//! ([`crate::ui_web::WebSuite`]), backed by the device sign-in in
//! [`crate::ui_client`]. Features `ui` and `client`.
//!
//! "Sign in" opens the device card (code, "Open browser", waiting). The
//! token lives in `credentials.json` (mode 0600) in the Avila config
//! directory. "Sign out" revokes that token on the service and forgets it
//! locally. "Create account" starts the device flow and, as soon as the
//! service has issued the code, opens `{base}/register?next=<approval page>`
//! so a new person lands on the approval page after registering.

use crate::account::{config_dir, Client};
use crate::ui::{self, Account, Launch, Launcher};
use crate::ui_client::{sign_in_panel, AccountState, DeviceLoginUi, SignInStatus};
use crate::ui_web::{
    account_base, account_chip, first_visit_card, manifest, page_url, prompt_visible,
    sign_in_button, ChipAction, PromptAction, WebState,
};
use egui::{Align, Id, Layout, Order, Ui};
use std::path::PathBuf;

/// The marker file that records a dismissed prompt. One file in the shared
/// config directory, so dismissing it in one desktop tool silences the
/// others: the account is shared too.
const DISMISS_FILE: &str = "signin-prompt-dismissed";

fn dismiss_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join(DISMISS_FILE))
}

/// The page that approves a device code: the complete URL the service
/// issued (`?code=` or `?user_code=`), else the verification page with the
/// code appended.
pub fn approval_url(verification_uri: &str, open_url: &str, user_code: &str) -> String {
    if open_url.contains("code=") {
        open_url.to_owned()
    } else {
        let sep = if verification_uri.contains('?') {
            '&'
        } else {
            '?'
        };
        format!("{verification_uri}{sep}user_code={user_code}")
    }
}

/// Opt-in self-screenshot for reviewing the controls without a display
/// session: `AVILA_PREVIEW_SHOT=out.png` saves the window after a few
/// frames and exits; `AVILA_PREVIEW_DEVICE_CARD=1` first opens the device
/// sign-in card. Unset in normal use.
struct Preview {
    path: PathBuf,
    device_card: bool,
    frame: u32,
    requested: bool,
}

impl Preview {
    fn from_env() -> Option<Preview> {
        let path = std::env::var_os("AVILA_PREVIEW_SHOT").filter(|p| !p.is_empty())?;
        Some(Preview {
            path: path.into(),
            device_card: std::env::var_os("AVILA_PREVIEW_DEVICE_CARD").is_some(),
            frame: 0,
            requested: false,
        })
    }
}

pub struct DesktopSuite {
    pub launcher: Launcher,
    base: String,
    account: AccountState,
    login: DeviceLoginUi,
    card_open: bool,
    menu_open: bool,
    /// "Create account" was pressed: open the registration page once the
    /// device code exists.
    register_when_ready: bool,
    dismissed: bool,
    /// Set after a failed sign-out or save; read with [`take_notice`](Self::take_notice).
    notice: Option<String>,
    preview: Option<Preview>,
}

impl DesktopSuite {
    /// `current` is this tool's id in the manifest (`actinv`, ...).
    pub fn new(ctx: &egui::Context, current: &str) -> Self {
        let base = account_base();
        Self {
            launcher: Launcher::new(manifest(), current),
            login: {
                let mut login = DeviceLoginUi::new(base.clone());
                login.app = format!("{current}-desktop");
                login
            },
            base,
            account: AccountState::new(ctx),
            card_open: false,
            menu_open: false,
            register_when_ready: false,
            dismissed: dismiss_path().is_some_and(|p| p.exists()),
            notice: None,
            preview: Preview::from_env(),
        }
    }

    /// The account as the controls see it.
    pub fn state(&self) -> WebState {
        match self.account.account() {
            Account::SignedIn { email, .. } => WebState::SignedIn { email },
            Account::SignedOut => WebState::SignedOut,
        }
    }

    /// A message worth showing in the host's status line, once.
    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }

    /// The launcher, then "Sign in" or the account chip, right to left.
    /// Call inside the host's header row.
    pub fn header_right(&mut self, ui: &mut Ui) {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.controls(ui));
    }

    /// The same controls for a ui that is already right to left.
    pub fn controls(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.x = 10.0;
        let mut launched = None;
        match self.state() {
            WebState::SignedOut | WebState::Unknown => {
                if sign_in_button(ui) {
                    self.start_sign_in(ui.ctx(), false);
                }
            }
            WebState::SignedIn { email } => match account_chip(ui, &email, &mut self.menu_open) {
                ChipAction::None => {}
                ChipAction::Dashboard => {
                    launched = Some(Launch::Dashboard(format!("{}/suite", self.base)));
                }
                ChipAction::SignOut => self.sign_out(),
            },
        }
        if let Some(l) = self.launcher.show(ui) {
            launched = Some(l);
        }
        if let Some(launch) = launched {
            ui::open(ui.ctx(), &launch);
        }
    }

    fn start_sign_in(&mut self, ctx: &egui::Context, register: bool) {
        self.card_open = true;
        self.register_when_ready = register;
        if !matches!(self.login.status(), SignInStatus::Waiting { .. }) {
            self.login.start(ctx);
        }
    }

    /// Revoke the token on the service (in the background) and forget it here.
    fn sign_out(&mut self) {
        if let Some(credentials) = self.account.credentials() {
            std::thread::spawn(move || {
                if let Ok(client) = Client::from_credentials(&credentials) {
                    let _ = client.logout();
                }
            });
        }
        if let Err(error) = self.account.sign_out() {
            self.notice = Some(format!("Account sign-out: {error}"));
        }
    }

    /// Same call as the browser build's `WebSuite::prompt`: the first-visit
    /// card and, while a sign-in is under way, the device
    /// card. Call once per frame after the header; `top` is the y just under
    /// it.
    pub fn prompt(&mut self, ctx: &egui::Context, top: f32) {
        self.show(ctx, top);
    }

    /// [`DesktopSuite::prompt`] under its fuller name.
    pub fn show(&mut self, ctx: &egui::Context, top: f32) {
        self.drive(ctx);
        self.preview_step(ctx);
        if self.card_open {
            self.device_card(ctx);
        } else if prompt_visible(&self.state(), self.dismissed) {
            match first_visit_card(ctx, top) {
                PromptAction::None => {}
                PromptAction::Create => self.start_sign_in(ctx, true),
                PromptAction::SignIn => self.start_sign_in(ctx, false),
                PromptAction::Dismiss => {
                    self.dismissed = true;
                    if let Some(path) = dismiss_path() {
                        if let Some(dir) = path.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = std::fs::write(path, "1\n");
                    }
                }
            }
        }
    }

    fn preview_step(&mut self, ctx: &egui::Context) {
        let Some(preview) = &mut self.preview else {
            return;
        };
        ctx.request_repaint();
        preview.frame += 1;
        // `AVILA_PREVIEW_THEME=dark|light` forces the egui theme for review.
        match std::env::var("AVILA_PREVIEW_THEME").as_deref() {
            Ok("dark") => ctx.set_theme(egui::ThemePreference::Dark),
            Ok("light") => ctx.set_theme(egui::ThemePreference::Light),
            _ => {}
        }
        if preview.device_card && preview.frame == 20 {
            self.card_open = true;
            self.login.start(ctx);
        }
        let Some(preview) = &mut self.preview else {
            return;
        };
        // Give the code request time to come back before capturing.
        let ready = preview.frame >= if preview.device_card { 150 } else { 60 };
        if ready && !preview.requested {
            preview.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = shot {
            let [w, h] = image.size;
            let pixels: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(buffer) = image::RgbaImage::from_raw(w as u32, h as u32, pixels) {
                let _ = buffer.save(&preview.path);
            }
            eprintln!("preview written: {}", preview.path.display());
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Per-frame bookkeeping: open the registration page once the code
    /// exists, and finish an approved sign-in.
    fn drive(&mut self, ctx: &egui::Context) {
        if self.register_when_ready {
            if let SignInStatus::Waiting {
                user_code,
                verification_uri,
                open_url,
            } = self.login.status()
            {
                self.register_when_ready = false;
                let next = approval_url(&verification_uri, &open_url, &user_code);
                ui::open(
                    ctx,
                    &Launch::SignIn(page_url(&self.base, "register", &next)),
                );
            }
        }
        match self.account.finish_sign_in(&mut self.login) {
            Ok(true) => self.card_open = false,
            Ok(false) => {}
            Err(error) => self.notice = Some(format!("Account sign-in: {error}")),
        }
    }

    fn device_card(&mut self, ctx: &egui::Context) {
        let mut launched = None;
        egui::Area::new(Id::new("avila-desktop-device-card"))
            .order(Order::Foreground)
            .fade_in(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                launched = sign_in_panel(ui, &mut self.login);
            });
        if let Some(launch) = launched {
            ui::open(ctx, &launch);
        }
        if self.login.take_close_request() {
            self.card_open = false;
            self.register_when_ready = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_url_prefers_the_complete_link() {
        assert_eq!(
            approval_url("http://h/device", "http://h/device?code=AB-12", "AB-12"),
            "http://h/device?code=AB-12"
        );
        assert_eq!(
            approval_url("http://h/device", "http://h/device", "AB-12"),
            "http://h/device?user_code=AB-12"
        );
    }

    #[test]
    fn register_next_carries_the_approval_page_encoded() {
        let next = approval_url("http://h/device", "http://h/device", "AB-12");
        assert_eq!(
            page_url("http://h", "register", &next),
            "http://h/register?next=http%3A%2F%2Fh%2Fdevice%3Fuser_code%3DAB-12"
        );
    }

    #[test]
    fn logout_posts_with_the_bearer_token_and_csrf_header() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let seen = std::thread::spawn(move || {
            let request = server.recv().unwrap();
            let header = |name: &'static str| {
                request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv(name))
                    .map(|h| h.value.to_string())
            };
            let seen = (
                request.method().to_string(),
                request.url().to_owned(),
                header("Authorization"),
                header("X-Avila-CSRF"),
            );
            request
                .respond(tiny_http::Response::from_string("{\"ok\":true}"))
                .unwrap();
            seen
        });
        Client::with_token(&base, "tok").unwrap().logout().unwrap();
        let (method, url, auth, csrf) = seen.join().unwrap();
        assert_eq!(
            (method.as_str(), url.as_str()),
            ("POST", "/api/auth/logout")
        );
        assert_eq!(auth.as_deref(), Some("Bearer tok"));
        assert_eq!(csrf.as_deref(), Some("1"));
    }
}
