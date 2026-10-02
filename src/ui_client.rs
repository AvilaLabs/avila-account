//! Account pieces for egui tools: [`AccountState`] (the stored sign-in,
//! re-checked in the background) and [`sign_in_panel`] (the device-code
//! sign-in). Features `ui` and `client`.
//!
//! Nothing here blocks the UI thread: network calls run on background
//! threads that ask egui for a repaint when something changed.

use crate::account::{Client, Credentials, DeviceLogin, LoginState, DEFAULT_BASE_URL};
use crate::ui::{self, eyebrow, pal, Account, Launch, Pal};
use egui::{vec2, Color32, CornerRadius, Frame, Margin, RichText, Stroke, Ui};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the saved sign-in is re-checked on its own.
pub const REFRESH_EVERY: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Snapshot {
    credentials: Option<Credentials>,
    /// The service refused the saved token.
    rejected: bool,
    /// The last failure worth showing (unreachable, refused).
    problem: Option<String>,
}

enum Message {
    Refresh,
    Stop,
}

/// The stored sign-in plus a poller that asks `/api/auth/me` whether the
/// service still accepts the token. Cheap to poll
/// from `update`: [`AccountState::account`] only takes a lock.
pub struct AccountState {
    shared: Arc<Mutex<Snapshot>>,
    wake: Sender<Message>,
}

impl AccountState {
    /// Load the saved credentials (a missing or unreadable file is signed
    /// out) and start polling every [`REFRESH_EVERY`].
    pub fn new(ctx: &egui::Context) -> AccountState {
        Self::with_interval(ctx, REFRESH_EVERY)
    }

    pub fn with_interval(ctx: &egui::Context, every: Duration) -> AccountState {
        let credentials = Credentials::load().ok().flatten();
        let shared = Arc::new(Mutex::new(Snapshot {
            credentials,
            ..Snapshot::default()
        }));
        let (wake, inbox) = mpsc::channel();
        let (thread_shared, ctx) = (shared.clone(), ctx.clone());
        std::thread::spawn(move || loop {
            fetch(&thread_shared, &ctx);
            match inbox.recv_timeout(every) {
                Ok(Message::Refresh) | Err(RecvTimeoutError::Timeout) => {}
                Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            }
        });
        AccountState { shared, wake }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Snapshot> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What the account chip shows.
    pub fn account(&self) -> Account {
        let snapshot = self.lock();
        match (&snapshot.credentials, snapshot.rejected) {
            (Some(credentials), false) if !credentials.email.is_empty() => Account::SignedIn {
                email: credentials.email.clone(),
            },
            (Some(_), false) => Account::SignedIn { email: "?".into() },
            _ => Account::SignedOut,
        }
    }

    /// Why the account is not usable right now: a refused token or an
    /// unreachable service.
    pub fn problem(&self) -> Option<String> {
        self.lock().problem.clone()
    }

    pub fn credentials(&self) -> Option<Credentials> {
        self.lock().credentials.clone()
    }

    /// Ask the poller to fetch now.
    pub fn refresh(&self) {
        let _ = self.wake.send(Message::Refresh);
    }

    /// Use these credentials (already saved by the caller, or not) and
    /// check them.
    pub fn set_credentials(&self, credentials: Credentials) {
        {
            let mut snapshot = self.lock();
            *snapshot = Snapshot {
                credentials: Some(credentials),
                ..Snapshot::default()
            };
        }
        self.refresh();
    }

    /// Save the credentials of a finished [`DeviceLoginUi`] and use them.
    /// Returns whether there was one.
    pub fn finish_sign_in(&self, login: &mut DeviceLoginUi) -> Result<bool, String> {
        let Some(credentials) = login.take_credentials() else {
            return Ok(false);
        };
        credentials.save().map_err(|e| e.to_string())?;
        self.set_credentials(credentials);
        Ok(true)
    }

    /// Delete the saved credentials.
    pub fn sign_out(&self) -> Result<(), String> {
        Credentials::clear().map_err(|e| e.to_string())?;
        *self.lock() = Snapshot::default();
        Ok(())
    }
}

impl Drop for AccountState {
    fn drop(&mut self) {
        let _ = self.wake.send(Message::Stop);
    }
}

fn fetch(shared: &Mutex<Snapshot>, ctx: &egui::Context) {
    let credentials = {
        let snapshot = shared.lock().unwrap_or_else(|e| e.into_inner());
        match &snapshot.credentials {
            Some(c) if !snapshot.rejected => c.clone(),
            _ => return,
        }
    };
    let outcome = Client::from_credentials(&credentials).and_then(|c| c.whoami());
    let mut snapshot = shared.lock().unwrap_or_else(|e| e.into_inner());
    // A newer sign-in replaced the one this fetch was made with.
    if snapshot.credentials.as_ref() != Some(&credentials) {
        return;
    }
    match outcome {
        Ok(_) => snapshot.problem = None,
        Err(crate::account::AccountError::Unauthorized) => {
            snapshot.rejected = true;
            snapshot.problem = Some(crate::account::AccountError::Unauthorized.to_string());
        }
        // A brief outage does not sign anyone out.
        Err(other) => snapshot.problem = Some(other.to_string()),
    }
    drop(snapshot);
    ctx.request_repaint();
}

// ---------------------------------------------------------------- sign in

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInStatus {
    Idle,
    /// Asking the service for a code.
    Starting,
    Waiting {
        user_code: String,
        verification_uri: String,
        /// The URL "Open browser" opens (carries the code when the service
        /// gave a complete one).
        open_url: String,
    },
    Approved {
        email: String,
    },
    Denied,
    Expired,
    Error(String),
}

/// State behind [`sign_in_panel`]. The device sign-in runs on a background
/// thread; the panel reads its status.
pub struct DeviceLoginUi {
    pub base_url: String,
    pub scope: String,
    /// The app named on the approval page (`actinv-desktop`, ...); empty
    /// uses the client's default name.
    pub app: String,
    status: Arc<Mutex<SignInStatus>>,
    approved: Arc<Mutex<Option<Credentials>>>,
    cancel: Arc<AtomicBool>,
    /// The person asked to dismiss the card (Close, or Cancel while waiting).
    close_requested: bool,
}

impl DeviceLoginUi {
    pub fn new(base_url: impl Into<String>) -> DeviceLoginUi {
        DeviceLoginUi {
            base_url: base_url.into(),
            scope: "read".into(),
            app: String::new(),
            status: Arc::new(Mutex::new(SignInStatus::Idle)),
            approved: Arc::new(Mutex::new(None)),
            cancel: Arc::new(AtomicBool::new(false)),
            close_requested: false,
        }
    }

    /// True once after the person pressed Close (or Cancel while waiting) in
    /// the card: the host hides its sign-in window. The card has no window
    /// chrome of its own, so hosts need no Close button outside it.
    pub fn take_close_request(&mut self) -> bool {
        std::mem::take(&mut self.close_requested)
    }

    /// Cancel any sign-in in progress and ask the host to hide the card.
    pub fn dismiss(&mut self) {
        self.cancel();
        self.close_requested = true;
    }

    /// The saved base URL, else `AVILA_BASE_URL`, else the account service
    /// default ([`crate::ui_web::account_base`]).
    pub fn from_environment() -> DeviceLoginUi {
        let base = Credentials::load()
            .ok()
            .flatten()
            .map(|c| c.base_url)
            .or_else(|| {
                std::env::var("AVILA_BASE_URL")
                    .ok()
                    .filter(|b| !b.is_empty())
            })
            .unwrap_or_else(crate::ui_web::account_base);
        Self::new(base)
    }

    /// A panel frozen in a status, with no network: for demos and tests.
    pub fn fake(status: SignInStatus) -> DeviceLoginUi {
        let panel = Self::new(DEFAULT_BASE_URL);
        *panel.status.lock().unwrap() = status;
        panel
    }

    pub fn status(&self) -> SignInStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Credentials of an approved sign-in, once.
    pub fn take_credentials(&mut self) -> Option<Credentials> {
        self.approved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Begin a sign-in on a background thread.
    pub fn start(&mut self, ctx: &egui::Context) {
        self.cancel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        *self.status.lock().unwrap() = SignInStatus::Starting;
        let (status, approved, ctx) = (self.status.clone(), self.approved.clone(), ctx.clone());
        let (base, scope, app) = (self.base_url.clone(), self.scope.clone(), self.app.clone());
        std::thread::spawn(move || {
            let set = |value: SignInStatus| {
                if !cancel.load(Ordering::SeqCst) {
                    *status.lock().unwrap_or_else(|e| e.into_inner()) = value;
                    ctx.request_repaint();
                }
            };
            let mut login = match if app.is_empty() {
                DeviceLogin::start(&base, &scope)
            } else {
                DeviceLogin::start_as(&base, &scope, &app)
            } {
                Ok(login) => login,
                Err(error) => return set(SignInStatus::Error(error.to_string())),
            };
            let code = login.code().clone();
            set(SignInStatus::Waiting {
                user_code: code.user_code.clone(),
                verification_uri: code.verification_uri.clone(),
                open_url: code.open_url().to_owned(),
            });
            loop {
                // Sleep in short steps so Cancel is prompt.
                let mut left = login.interval();
                while left > Duration::ZERO && !cancel.load(Ordering::SeqCst) {
                    let step = left.min(Duration::from_millis(100));
                    std::thread::sleep(step);
                    left -= step;
                }
                if cancel.load(Ordering::SeqCst) {
                    return;
                }
                match login.poll_once().clone() {
                    LoginState::Pending => {}
                    LoginState::Approved(credentials) => {
                        let email = credentials.email.clone();
                        *approved.lock().unwrap_or_else(|e| e.into_inner()) = Some(credentials);
                        return set(SignInStatus::Approved { email });
                    }
                    LoginState::Denied => return set(SignInStatus::Denied),
                    LoginState::Expired => return set(SignInStatus::Expired),
                    LoginState::Error(message) => return set(SignInStatus::Error(message)),
                }
            }
        });
    }

    /// Stop polling and return to idle.
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let mut status = self.status.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(
            *status,
            SignInStatus::Starting | SignInStatus::Waiting { .. }
        ) {
            *status = SignInStatus::Idle;
        }
    }
}

impl Drop for DeviceLoginUi {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

fn primary(text: &str, p: Pal) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(text.to_owned())
            .size(13.0)
            .strong()
            .color(Color32::WHITE),
    )
    .fill(p.primary)
    .corner_radius(CornerRadius::ZERO)
    .min_size(vec2(120.0, 32.0))
}

fn secondary(text: &str, p: Pal) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(text.to_owned())
            .size(13.0)
            .strong()
            .color(p.accent),
    )
    .fill(p.card)
    .stroke(Stroke::new(1.0, p.accent))
    .corner_radius(CornerRadius::ZERO)
    .min_size(vec2(90.0, 32.0))
}

/// The device sign-in card: the code large in monospace, an "Open browser"
/// button, a waiting indicator and the result. Returns
/// [`Launch::SignIn`] with the URL to open when the button is clicked (the
/// host passes it to [`ui::open`]).
pub fn sign_in_panel(ui: &mut Ui, login: &mut DeviceLoginUi) -> Option<Launch> {
    let mut launched = None;
    let status = login.status();
    let p = pal(ui.visuals().dark_mode);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.line))
        .inner_margin(Margin::same(24))
        .show(ui, |ui| {
            ui.set_width(400.0);
            // Hosts often centre their content; the card reads left-aligned.
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.label(eyebrow("Sign in to your Avila account").color(p.accent));
            ui.add_space(10.0);
            match &status {
                SignInStatus::Idle | SignInStatus::Starting => {
                    ui.label(
                        RichText::new("Approve this device in your browser. Every tool keeps working without an account.")
                            .size(13.0)
                            .color(p.muted),
                    );
                    ui.add_space(14.0);
                    let starting = status == SignInStatus::Starting;
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!starting, primary("Start sign-in", p)).clicked() {
                            login.start(ui.ctx());
                        }
                        if starting {
                            ui.add(egui::Spinner::new().size(16.0).color(p.accent));
                            ui.label(RichText::new("Contacting the service").size(12.5).color(p.muted));
                        }
                        if ui.add(secondary("Close", p)).clicked() {
                            login.dismiss();
                        }
                    });
                }
                SignInStatus::Waiting {
                    user_code,
                    verification_uri,
                    open_url,
                } => {
                    ui.label(
                        RichText::new("Your browser will open and ask you to approve this sign-in. Check that it shows this code.")
                            .size(13.0)
                            .color(p.muted),
                    );
                    ui.add_space(10.0);
                    Frame::new()
                        .fill(p.tint)
                        .inner_margin(Margin::symmetric(0, 16))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(user_code)
                                        .monospace()
                                        .size(34.0)
                                        .strong()
                                        .extra_letter_spacing(3.0)
                                        .color(p.accent),
                                );
                            });
                        });
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("at {verification_uri}"))
                            .monospace()
                            .size(11.5)
                            .color(p.muted),
                    );
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if ui.add(primary("Open browser", p)).clicked() {
                            launched = Some(Launch::SignIn(open_url.clone()));
                        }
                        if ui.add(secondary("Cancel", p)).clicked() {
                            login.dismiss();
                        }
                    });
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(16.0).color(p.accent));
                        ui.label(
                            RichText::new("Waiting for approval").size(12.5).color(p.ink),
                        );
                    });
                }
                SignInStatus::Approved { email } => {
                    let who = if email.is_empty() { "your account" } else { email };
                    ui.label(
                        RichText::new(format!("Signed in as {who}"))
                            .size(15.0)
                            .strong()
                            .color(p.ink),
                    );
                }
                SignInStatus::Denied => outcome(ui, p, login, "The sign-in was denied.", true),
                SignInStatus::Expired => {
                    outcome(ui, p, login, "The code expired before it was approved.", true)
                }
                SignInStatus::Error(message) => outcome(ui, p, login, message, true),
            }
            });
        });
    launched
}

fn outcome(ui: &mut Ui, p: Pal, login: &mut DeviceLoginUi, message: &str, retry: bool) {
    ui.label(RichText::new(message).size(13.0).color(ui::ALERT));
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if retry && ui.add(primary("Try again", p)).clicked() {
            login.start(ui.ctx());
        }
        if ui.add(secondary("Close", p)).clicked() {
            login.dismiss();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn serve(handler: impl Fn(&str, Option<&str>) -> (u16, String) + Send + 'static) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut sink = Vec::new();
                let _ = std::io::Read::read_to_end(request.as_reader(), &mut sink);
                let auth = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .map(|h| h.value.to_string());
                let (status, body) = handler(request.url(), auth.as_deref());
                let _ = request
                    .respond(tiny_http::Response::from_string(body).with_status_code(status));
            }
        });
        base
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out: {what}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn creds(base: &str, token: &str) -> Credentials {
        Credentials {
            base_url: base.into(),
            token: token.into(),
            email: "a@example.org".into(),
            scope: "read".into(),
        }
    }

    #[test]
    fn dismiss_cancels_and_asks_the_host_once() {
        let mut login = DeviceLoginUi::fake(SignInStatus::Waiting {
            user_code: "ABCD-EFGH".into(),
            verification_uri: "http://127.0.0.1:1/device".into(),
            open_url: "http://127.0.0.1:1/device".into(),
        });
        assert!(!login.take_close_request());
        login.dismiss();
        assert_eq!(login.status(), SignInStatus::Idle);
        assert!(login.take_close_request());
        assert!(!login.take_close_request(), "the request is taken once");
        // A plain cancel (for example a new start) does not close the card.
        login.cancel();
        assert!(!login.take_close_request());
    }

    #[test]
    fn account_state_checks_the_token_and_notices_a_revoked_one() {
        let base = serve(|url, auth| match (url, auth) {
            ("/api/auth/me", Some("Bearer good")) => (
                200,
                r#"{"user":{"email":"a@example.org","email_verified":true}}"#.into(),
            ),
            _ => (401, r#"{"error":"unauthorized"}"#.into()),
        });
        let ctx = egui::Context::default();
        let state = AccountState::with_interval(&ctx, Duration::from_millis(40));
        state.set_credentials(creds(&base, "good"));
        wait_for("checked", || {
            state.account()
                == Account::SignedIn {
                    email: "a@example.org".into(),
                }
        });
        assert!(state.problem().is_none());
        state.set_credentials(creds(&base, "revoked"));
        wait_for("rejected", || state.account() == Account::SignedOut);
        assert!(state.problem().unwrap().contains("sign in again"));
    }

    #[test]
    fn device_login_ui_runs_to_approval() {
        let polls = Mutex::new(0);
        let base = serve(move |url, _| {
            match url {
            "/api/device/code" => (
                200,
                r#"{"device_code":"d","user_code":"WXYZ-1234","verification_uri":"http://127.0.0.1/device","verification_uri_complete":"http://127.0.0.1/device?code=WXYZ-1234","expires_in":600,"interval":0}"#.into(),
            ),
            "/api/device/token" => {
                let mut n = polls.lock().unwrap();
                *n += 1;
                if *n < 3 {
                    (400, r#"{"error":"authorization_pending"}"#.into())
                } else {
                    (200, r#"{"access_token":"tok","token_type":"Bearer","scope":"read"}"#.into())
                }
            }
            "/api/auth/me" => (
                200,
                r#"{"user":{"email":"a@example.org","email_verified":true}}"#.into(),
            ),
            _ => (404, "{}".into()),
        }
        });
        let ctx = egui::Context::default();
        let mut login = DeviceLoginUi::new(base);
        assert_eq!(login.status(), SignInStatus::Idle);
        login.start(&ctx);
        wait_for("approved", || {
            matches!(login.status(), SignInStatus::Approved { .. })
        });
        assert_eq!(
            login.status(),
            SignInStatus::Approved {
                email: "a@example.org".into()
            }
        );
        let credentials = login.take_credentials().unwrap();
        assert_eq!(credentials.token, "tok");
        assert!(login.take_credentials().is_none());
    }

    #[test]
    fn cancel_stops_polling_and_service_errors_are_shown() {
        let base = serve(|url, _| {
            match url {
            "/api/device/code" => (
                200,
                r#"{"device_code":"d","user_code":"AAAA-BBBB","verification_uri":"http://127.0.0.1/device","expires_in":600,"interval":0}"#.into(),
            ),
            _ => (400, r#"{"error":"authorization_pending"}"#.into()),
        }
        });
        let ctx = egui::Context::default();
        let mut login = panel_for(&base);
        login.start(&ctx);
        wait_for("waiting", || {
            matches!(login.status(), SignInStatus::Waiting { .. })
        });
        login.cancel();
        assert_eq!(login.status(), SignInStatus::Idle);
        let mut dead = DeviceLoginUi::new("http://127.0.0.1:1");
        dead.start(&ctx);
        wait_for("error", || matches!(dead.status(), SignInStatus::Error(_)));
    }

    #[test]
    fn the_two_default_service_urls_agree() {
        assert_eq!(DEFAULT_BASE_URL, crate::ui_web::DEFAULT_BASE);
    }

    fn panel_for(base: &str) -> DeviceLoginUi {
        DeviceLoginUi::new(base)
    }

    #[test]
    fn the_panel_draws_every_status_without_the_network() {
        let ctx = egui::Context::default();
        for status in [
            SignInStatus::Idle,
            SignInStatus::Waiting {
                user_code: "ABCD-EFGH".into(),
                verification_uri: "http://127.0.0.1:8787/device".into(),
                open_url: "http://127.0.0.1:8787/device?code=ABCD-EFGH".into(),
            },
            SignInStatus::Approved {
                email: "a@example.org".into(),
            },
            SignInStatus::Denied,
            SignInStatus::Expired,
            SignInStatus::Error("cannot reach the service".into()),
        ] {
            let mut login = DeviceLoginUi::fake(status);
            let mut output = ctx.run_ui(Default::default(), |ui| {
                assert!(sign_in_panel(ui, &mut login).is_none());
            });
            output.textures_delta.clear();
        }
    }
}
