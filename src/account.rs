//! Account client: device-code sign-in, the credentials file, and the few
//! calls a tool makes against the Avila Labs account service. Feature
//! `client`.
//!
//! The service contract this codes against:
//!
//! * `POST /api/device/code {client, scope}` starts a sign-in;
//! * `POST /api/device/token {device_code}` polls it (200 with a token, or
//!   400 `authorization_pending` / `slow_down` / `access_denied` /
//!   `expired_token`);
//! * `GET /api/auth/me` and `POST /api/auth/logout`, with a Bearer token.
//!
//! The token is never printed: `Debug` for [`Credentials`] and [`Client`]
//! redacts it, and error messages never include it. A token is only ever
//! sent to the base URL it was issued by, and plain `http` is accepted only
//! for a loopback host.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The account service, unless `AVILA_ACCOUNT_BASE` was set at build time
/// (see [`crate::ui_web::account_base`]).
pub const DEFAULT_BASE_URL: &str = "https://api.avilalabs.org";
/// The client name sent when a device sign-in does not name an app. The
/// service accepts only a fixed set of names.
pub const CLIENT_NAME: &str = "avila-suite";
/// Scopes this client asks for.
pub const SCOPES: [&str; 1] = ["read"];

#[derive(Debug, PartialEq, Eq)]
pub enum AccountError {
    /// No credentials file.
    NotSignedIn,
    /// The service refused the token (401): revoked, expired or wrong host.
    Unauthorized,
    /// The service answered with another error status.
    Http { status: u16, message: String },
    /// The service could not be reached.
    Network(String),
    /// The service answered something the contract does not describe.
    Protocol(String),
    /// Credentials file or local file problem.
    Io(String),
    /// A base URL or scope the client will not use.
    Invalid(String),
}

impl fmt::Display for AccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AccountError::NotSignedIn => f.write_str("not signed in"),
            AccountError::Unauthorized => f.write_str(
                "the service rejected the saved sign-in (the token was revoked or has expired); \
                 sign in again",
            ),
            AccountError::Http { status, message } => {
                write!(f, "the service answered {status}: {message}")
            }
            AccountError::Network(detail) => write!(f, "cannot reach the service: {detail}"),
            AccountError::Protocol(detail) => write!(f, "unexpected service reply: {detail}"),
            AccountError::Io(detail) => f.write_str(detail),
            AccountError::Invalid(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for AccountError {}

// ------------------------------------------------------------- credentials

/// What a finished sign-in saves. `Debug` never shows the token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub base_url: String,
    pub token: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub scope: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("base_url", &self.base_url)
            .field("token", &"<redacted>")
            .field("email", &self.email)
            .field("scope", &self.scope)
            .finish()
    }
}

/// The directory holding `credentials.json`: `AVILA_CONFIG_DIR`, else
/// `~/.config/avila`.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("AVILA_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".config").join("avila"))
}

impl Credentials {
    pub fn path() -> Result<PathBuf, AccountError> {
        config_dir()
            .map(|d| d.join("credentials.json"))
            .ok_or_else(|| AccountError::Io("no home directory; set AVILA_CONFIG_DIR".into()))
    }

    /// `Ok(None)` when there is no file.
    pub fn load() -> Result<Option<Credentials>, AccountError> {
        Self::load_from(&Self::path()?)
    }

    pub fn load_from(path: &Path) -> Result<Option<Credentials>, AccountError> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| {
                AccountError::Io(format!("{}: not valid credentials ({e})", path.display()))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(AccountError::Io(format!("{}: {e}", path.display()))),
        }
    }

    pub fn save(&self) -> Result<PathBuf, AccountError> {
        let path = Self::path()?;
        self.save_to(&path)?;
        Ok(path)
    }

    /// Write the file with mode 0600 (unix), replacing any earlier one
    /// atomically so a reader never sees a half-written token.
    pub fn save_to(&self, path: &Path) -> Result<(), AccountError> {
        let io = |e: std::io::Error| AccountError::Io(format!("{}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let temp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).expect("credentials serialize");
        {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp).map_err(io)?;
            file.write_all(text.as_bytes()).map_err(io)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600)).map_err(io)?;
        }
        std::fs::rename(&temp, path).map_err(io)
    }

    /// Delete the file. `Ok(false)` when there was none.
    pub fn clear() -> Result<bool, AccountError> {
        Self::clear_at(&Self::path()?)
    }

    pub fn clear_at(path: &Path) -> Result<bool, AccountError> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(AccountError::Io(format!("{}: {e}", path.display()))),
        }
    }
}

/// Validate and normalize a base URL: `https://host[:port]`, or `http://`
/// only for a loopback host; no trailing slash.
pub fn normalize_base(url: &str) -> Result<String, AccountError> {
    let url = url.trim().trim_end_matches('/');
    let (scheme, rest) = url.split_once("://").ok_or_else(|| {
        AccountError::Invalid(format!("{url:?} is not a URL (http:// or https://)"))
    })?;
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(AccountError::Invalid(format!("{url:?} has no usable host")));
    }
    if rest.contains('/') {
        return Err(AccountError::Invalid(
            "the base URL is a host, without a path".into(),
        ));
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    match scheme {
        "https" => {}
        "http" => {
            let loopback = host == "localhost" || host == "::1" || host.starts_with("127.");
            if !loopback {
                return Err(AccountError::Invalid(format!(
                    "refusing to send a sign-in over plain http to {host}; use https"
                )));
            }
        }
        other => {
            return Err(AccountError::Invalid(format!(
                "unsupported URL scheme {other:?}"
            )))
        }
    }
    Ok(url.to_owned())
}

/// `read read` → `read`; unknown or empty scope names refused.
pub fn normalize_scope(text: &str) -> Result<String, AccountError> {
    let mut names: Vec<&str> = Vec::new();
    for name in text.split([',', ' ']).filter(|n| !n.is_empty()) {
        if !SCOPES.contains(&name) {
            return Err(AccountError::Invalid(format!(
                "unknown scope {name:?}; use {}",
                SCOPES.join(", ")
            )));
        }
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if names.is_empty() {
        return Err(AccountError::Invalid("scope is empty".into()));
    }
    Ok(names.join(","))
}

// ------------------------------------------------------------------ client

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub interval: u64,
}

impl DeviceCode {
    /// The URL to open: the complete one when the service gave it.
    pub fn open_url(&self) -> &str {
        self.verification_uri_complete
            .as_deref()
            .unwrap_or(&self.verification_uri)
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct TokenGrant {
    pub access_token: String,
    #[serde(default)]
    pub scope: String,
}

impl fmt::Debug for TokenGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenGrant")
            .field("access_token", &"<redacted>")
            .field("scope", &self.scope)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenPoll {
    Approved(TokenGrant),
    Pending,
    SlowDown,
    Denied,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WhoamiUser {
    pub email: String,
    #[serde(default)]
    pub email_verified: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Whoami {
    pub user: WhoamiUser,
}

pub struct Client {
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Client {
    /// A client for a base URL, without a token (sign-in calls).
    pub fn new(base_url: &str) -> Result<Client, AccountError> {
        Ok(Client {
            base: normalize_base(base_url)?,
            token: None,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .user_agent(concat!("avila-account/", env!("CARGO_PKG_VERSION")))
                .redirects(0)
                .build(),
        })
    }

    pub fn with_token(base_url: &str, token: &str) -> Result<Client, AccountError> {
        let mut client = Client::new(base_url)?;
        client.token = Some(token.to_owned());
        Ok(client)
    }

    pub fn from_credentials(credentials: &Credentials) -> Result<Client, AccountError> {
        Client::with_token(&credentials.base_url, &credentials.token)
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn send(&self, method: &str, path: &str) -> Result<Value, AccountError> {
        let text = self.send_text(method, path, &[], None)?;
        serde_json::from_str::<Value>(&text)
            .map_err(|e| AccountError::Protocol(format!("{path}: reply is not JSON ({e})")))
    }

    fn send_text(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<(&str, &[u8])>,
    ) -> Result<String, AccountError> {
        let mut request = self.agent.request(method, &format!("{}{path}", self.base));
        if let Some(token) = &self.token {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        for (name, value) in headers {
            request = request.set(name, value);
        }
        let result = match body {
            Some((content_type, bytes)) => {
                request.set("Content-Type", content_type).send_bytes(bytes)
            }
            None => request.call(),
        };
        match result {
            Ok(response) => response
                .into_string()
                .map_err(|e| AccountError::Protocol(format!("{path}: unreadable reply ({e})"))),
            Err(ureq::Error::Status(401, _)) if self.token.is_some() => {
                Err(AccountError::Unauthorized)
            }
            Err(ureq::Error::Status(status, response)) => {
                let text = response.into_string().unwrap_or_default();
                Err(AccountError::Http {
                    status,
                    message: error_message(&text),
                })
            }
            Err(ureq::Error::Transport(t)) => Err(AccountError::Network(t.to_string())),
        }
    }

    fn post_json(&self, path: &str, body: &Value) -> Result<Value, AccountError> {
        let text = self.send_text(
            "POST",
            path,
            &[],
            Some(("application/json", body.to_string().as_bytes())),
        )?;
        serde_json::from_str::<Value>(&text)
            .map_err(|e| AccountError::Protocol(format!("{path}: reply is not JSON ({e})")))
    }

    pub fn device_code(&self, scope: &str) -> Result<DeviceCode, AccountError> {
        self.device_code_as(CLIENT_NAME, scope)
    }

    /// [`Client::device_code`] naming the app (`actinv-desktop`, ...), so
    /// the approval page can say which app is asking.
    pub fn device_code_as(&self, client: &str, scope: &str) -> Result<DeviceCode, AccountError> {
        let reply = self.post_json(
            "/api/device/code",
            &json!({"client": client, "scope": normalize_scope(scope)?}),
        )?;
        serde_json::from_value(reply)
            .map_err(|e| AccountError::Protocol(format!("/api/device/code: {e}")))
    }

    pub fn device_token(&self, device_code: &str) -> Result<TokenPoll, AccountError> {
        match self.post_json("/api/device/token", &json!({"device_code": device_code})) {
            Ok(reply) => serde_json::from_value(reply)
                .map(TokenPoll::Approved)
                .map_err(|e| AccountError::Protocol(format!("/api/device/token: {e}"))),
            Err(AccountError::Http {
                status: 400,
                message,
            }) => match message.as_str() {
                "authorization_pending" => Ok(TokenPoll::Pending),
                "slow_down" => Ok(TokenPoll::SlowDown),
                "access_denied" => Ok(TokenPoll::Denied),
                "expired_token" => Ok(TokenPoll::Expired),
                other => Err(AccountError::Protocol(format!(
                    "/api/device/token: unknown error {other:?}"
                ))),
            },
            Err(other) => Err(other),
        }
    }

    pub fn whoami(&self) -> Result<Whoami, AccountError> {
        let reply = self.send("GET", "/api/auth/me")?;
        serde_json::from_value(reply)
            .map_err(|e| AccountError::Protocol(format!("/api/auth/me: {e}")))
    }

    /// `POST /api/auth/logout`: the service revokes the bearer token this
    /// client holds. The reply body is not used.
    pub fn logout(&self) -> Result<(), AccountError> {
        self.send_text(
            "POST",
            "/api/auth/logout",
            &[("X-Avila-CSRF", "1")],
            Some(("application/json", b"{}")),
        )
        .map(|_| ())
    }
}

/// The `error` field of a JSON error body, else the text itself, capped.
fn error_message(text: &str) -> String {
    let message = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| match v.get("error") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) => Some(other.to_string()),
            None => v.get("message").and_then(Value::as_str).map(str::to_owned),
        })
        .unwrap_or_else(|| text.trim().to_owned());
    message.chars().take(300).collect()
}

// ------------------------------------------------------------ device login

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginState {
    Pending,
    /// Signed in; the caller saves the credentials.
    Approved(Credentials),
    Denied,
    Expired,
    Error(String),
}

impl LoginState {
    pub fn is_final(&self) -> bool {
        !matches!(self, LoginState::Pending)
    }
}

/// One device sign-in. Drive it with [`DeviceLogin::wait`] (blocking)
/// or by calling [`DeviceLogin::poll_once`] every [`DeviceLogin::interval`]
/// from a background thread (egui).
pub struct DeviceLogin {
    client: Client,
    code: DeviceCode,
    scope: String,
    interval: Duration,
    expires_at: Instant,
    transport_failures: u32,
    state: LoginState,
}

/// Seconds added to the interval each time the service says `slow_down`.
pub const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);
/// Poll intervals below this are raised to it.
const MIN_INTERVAL: Duration = Duration::from_secs(0);
/// Consecutive unreachable-service polls tolerated before giving up.
const MAX_TRANSPORT_FAILURES: u32 = 3;

impl DeviceLogin {
    pub fn start(base_url: &str, scope: &str) -> Result<DeviceLogin, AccountError> {
        Self::start_as(base_url, scope, CLIENT_NAME)
    }

    /// [`DeviceLogin::start`] naming the app that asks.
    pub fn start_as(base_url: &str, scope: &str, app: &str) -> Result<DeviceLogin, AccountError> {
        let client = Client::new(base_url)?;
        let scope = normalize_scope(scope)?;
        let code = client.device_code_as(app, &scope)?;
        Ok(DeviceLogin::from_code(client, code, scope))
    }

    /// Continue with a code already obtained.
    pub fn from_code(client: Client, code: DeviceCode, scope: String) -> DeviceLogin {
        DeviceLogin {
            interval: Duration::from_secs(code.interval).max(MIN_INTERVAL),
            expires_at: Instant::now() + Duration::from_secs(code.expires_in),
            client,
            code,
            scope,
            transport_failures: 0,
            state: LoginState::Pending,
        }
    }

    pub fn code(&self) -> &DeviceCode {
        &self.code
    }

    pub fn state(&self) -> &LoginState {
        &self.state
    }

    /// How long to wait before the next poll (grows on `slow_down`).
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// One poll. Does nothing once the state is final.
    pub fn poll_once(&mut self) -> &LoginState {
        if self.state.is_final() {
            return &self.state;
        }
        if Instant::now() >= self.expires_at {
            self.state = LoginState::Expired;
            return &self.state;
        }
        match self.client.device_token(&self.code.device_code) {
            Ok(TokenPoll::Pending) => self.transport_failures = 0,
            Ok(TokenPoll::SlowDown) => {
                self.transport_failures = 0;
                self.interval += SLOW_DOWN_STEP;
            }
            Ok(TokenPoll::Denied) => self.state = LoginState::Denied,
            Ok(TokenPoll::Expired) => self.state = LoginState::Expired,
            Ok(TokenPoll::Approved(grant)) => self.state = self.approved(grant),
            Err(AccountError::Network(detail)) => {
                self.transport_failures += 1;
                if self.transport_failures >= MAX_TRANSPORT_FAILURES {
                    self.state = LoginState::Error(format!("cannot reach the service: {detail}"));
                }
            }
            Err(other) => self.state = LoginState::Error(other.to_string()),
        }
        &self.state
    }

    fn approved(&self, grant: TokenGrant) -> LoginState {
        let scope = if grant.scope.is_empty() {
            self.scope.clone()
        } else {
            grant.scope.clone()
        };
        // The email is a courtesy for display; a failure here does not undo
        // a sign-in the user has already approved.
        let email = Client::with_token(self.client.base_url(), &grant.access_token)
            .and_then(|c| c.whoami())
            .map(|w| w.user.email)
            .unwrap_or_default();
        LoginState::Approved(Credentials {
            base_url: self.client.base_url().to_owned(),
            token: grant.access_token,
            email,
            scope,
        })
    }

    /// Poll until the state is final, waiting with `sleep` between polls.
    pub fn wait_with(&mut self, sleep: &mut dyn FnMut(Duration)) -> LoginState {
        while !self.state.is_final() {
            sleep(self.interval);
            self.poll_once();
        }
        self.state.clone()
    }

    /// [`DeviceLogin::wait_with`] using `std::thread::sleep`.
    pub fn wait(&mut self) -> LoginState {
        self.wait_with(&mut std::thread::sleep)
    }
}

/// Try to open a URL in the default browser. Never fails: returns whether a
/// launcher was started.
pub fn open_browser(url: &str) -> bool {
    // `AVILA_NO_BROWSER=1` keeps tests and headless shells from launching one.
    if std::env::var_os("AVILA_NO_BROWSER").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    // Only web URLs, so a hostile service cannot make the launcher open a
    // local file or another scheme.
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return false;
    }
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("open", &[])]
    } else if cfg!(target_os = "windows") {
        &[("cmd", &["/C", "start", ""])]
    } else {
        &[("xdg-open", &[])]
    };
    candidates.iter().any(|(program, prefix)| {
        std::process::Command::new(program)
            .args(*prefix)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
    })
}
