//! The account client against a tiny in-process mock of the service
//! contract (127.0.0.1, no real network).

#![cfg(feature = "client")]

use avila_account::account::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

struct Mock {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Mock {
    /// `handler` gets each request and returns (status, JSON body).
    fn start(handler: impl Fn(&Seen) -> (u16, Value) + Send + 'static) -> Mock {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = Vec::new();
                std::io::Read::read_to_end(request.as_reader(), &mut body).unwrap();
                let record = Seen {
                    method: request.method().to_string(),
                    url: request.url().to_owned(),
                    headers: request
                        .headers()
                        .iter()
                        .map(|h| (h.field.to_string(), h.value.to_string()))
                        .collect(),
                    body,
                };
                let (status, reply) = handler(&record);
                log.lock().unwrap().push(record);
                let response = tiny_http::Response::from_string(reply.to_string())
                    .with_status_code(status)
                    .with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                    );
                let _ = request.respond(response);
            }
        });
        Mock { base, seen }
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

const TOKEN: &str = "avl_secret_token_value";

fn device_reply() -> Value {
    json!({
        "device_code": "dev-123", "user_code": "ABCD-EFGH",
        "verification_uri": "http://127.0.0.1/device",
        "verification_uri_complete": "http://127.0.0.1/device?code=ABCD-EFGH",
        "expires_in": 600, "interval": 0
    })
}

/// A service that answers the token poll from `script`, one entry per call
/// (the last repeats).
fn device_service(script: Vec<(u16, Value)>) -> Mock {
    let calls = Mutex::new(0usize);
    Mock::start(move |req| match req.url.as_str() {
        "/api/device/code" => (200, device_reply()),
        "/api/device/token" => {
            let mut n = calls.lock().unwrap();
            let entry = script[(*n).min(script.len() - 1)].clone();
            *n += 1;
            entry
        }
        "/api/auth/me" if req.header("authorization") == Some(&format!("Bearer {TOKEN}")) => (
            200,
            json!({"user": {"email": "a@example.org", "email_verified": true},
                   "owner": "user:1"}),
        ),
        _ => (404, json!({"error": "not found"})),
    })
}

fn pending() -> (u16, Value) {
    (400, json!({"error": "authorization_pending"}))
}
fn approved() -> (u16, Value) {
    (
        200,
        json!({"access_token": TOKEN, "token_type": "Bearer", "scope": "read"}),
    )
}

#[test]
fn device_flow_honors_slow_down_and_returns_credentials() {
    let mock = device_service(vec![
        pending(),
        (400, json!({"error": "slow_down"})),
        pending(),
        approved(),
    ]);
    let mut login = DeviceLogin::start(&mock.base, "read, read").unwrap();
    assert_eq!(login.code().user_code, "ABCD-EFGH");
    assert_eq!(
        login.code().open_url(),
        "http://127.0.0.1/device?code=ABCD-EFGH"
    );
    let mut slept = Vec::new();
    let state = login.wait_with(&mut |d| slept.push(d));
    let secs = |n| Duration::from_secs(n);
    assert_eq!(slept, [secs(0), secs(0), secs(5), secs(5)]);
    match state {
        LoginState::Approved(credentials) => {
            assert_eq!(credentials.token, TOKEN);
            assert_eq!(credentials.email, "a@example.org");
            assert_eq!(credentials.scope, "read");
            assert_eq!(credentials.base_url, mock.base);
            assert!(!format!("{credentials:?}").contains(TOKEN));
        }
        other => panic!("{other:?}"),
    }
    let requests = mock.requests();
    assert_eq!(
        requests[0].json(),
        json!({"client": "avila-suite", "scope": "read"})
    );
    assert_eq!(requests[1].json(), json!({"device_code": "dev-123"}));
    // Polling stops once approved.
    let after = login.poll_once().clone();
    assert_eq!(&after, login.state());
    assert_eq!(mock.requests().len(), 6);
}

#[test]
fn device_flow_denied_expired_and_errors() {
    for (reply, expect) in [
        ((400, json!({"error": "access_denied"})), LoginState::Denied),
        (
            (400, json!({"error": "expired_token"})),
            LoginState::Expired,
        ),
    ] {
        let mock = device_service(vec![pending(), reply]);
        let mut login = DeviceLogin::start(&mock.base, "read").unwrap();
        assert_eq!(login.wait_with(&mut |_| {}), expect);
    }
    let mock = device_service(vec![(400, json!({"error": "mystery"}))]);
    let mut login = DeviceLogin::start(&mock.base, "read").unwrap();
    assert!(matches!(login.poll_once(), LoginState::Error(m) if m.contains("mystery")));
    let mock = device_service(vec![(500, json!({"error": "boom"}))]);
    let mut login = DeviceLogin::start(&mock.base, "read").unwrap();
    assert!(matches!(login.poll_once(), LoginState::Error(m) if m.contains("500")));
}

#[test]
fn device_login_expires_locally_and_survives_brief_outages() {
    let mock = device_service(vec![pending()]);
    let client = Client::new(&mock.base).unwrap();
    let mut code: DeviceCode = client.device_code("read").unwrap();
    code.expires_in = 0;
    let mut login = DeviceLogin::from_code(client, code, "read".into());
    assert_eq!(login.poll_once(), &LoginState::Expired);
    let polls = mock
        .requests()
        .iter()
        .filter(|r| r.url.ends_with("token"))
        .count();
    assert_eq!(polls, 0, "an expired code is not sent");

    // Service unreachable: pending for two polls, an error at the third.
    let mock = device_service(vec![pending()]);
    let client = Client::new(&mock.base).unwrap();
    let code = client.device_code("read").unwrap();
    let dead = Client::new("http://127.0.0.1:1").unwrap();
    let mut login = DeviceLogin::from_code(dead, code, "read".into());
    assert_eq!(login.poll_once(), &LoginState::Pending);
    assert_eq!(login.poll_once(), &LoginState::Pending);
    assert!(matches!(login.poll_once(), LoginState::Error(_)));
}

#[test]
fn scope_and_base_validation() {
    assert_eq!(normalize_scope("read read").unwrap(), "read");
    assert!(normalize_scope("compute").is_err());
    assert!(normalize_scope("admin").is_err());
    assert!(normalize_scope(" ").is_err());
    assert_eq!(
        normalize_base("http://127.0.0.1:8787/").unwrap(),
        "http://127.0.0.1:8787"
    );
    assert!(normalize_base("http://localhost:1").is_ok());
    assert!(normalize_base("http://[::1]:1").is_ok());
    assert!(normalize_base("https://avila.example").is_ok());
    for bad in [
        "http://example.org",
        "ftp://127.0.0.1",
        "127.0.0.1:8787",
        "https://user@host",
        "https://host/path",
        "https://",
    ] {
        assert!(normalize_base(bad).is_err(), "{bad}");
    }
}

fn signed_in_service() -> Mock {
    Mock::start(|req| {
        if req.header("authorization") != Some(&format!("Bearer {TOKEN}")) {
            return (401, json!({"error": "unauthorized"}));
        }
        match (req.method.as_str(), req.url.as_str()) {
            ("GET", "/api/auth/me") => (
                200,
                json!({"user": {"email": "a@example.org", "email_verified": false},
                       "owner": "user:1"}),
            ),
            ("POST", "/api/auth/logout") => (200, json!({"ok": true})),
            _ => (404, json!({"error": "not found"})),
        }
    })
}

#[test]
fn whoami_and_logout_send_the_bearer_token() {
    let mock = signed_in_service();
    let client = Client::with_token(&mock.base, TOKEN).unwrap();
    let me = client.whoami().unwrap();
    assert_eq!(me.user.email, "a@example.org");
    assert!(!me.user.email_verified);
    client.logout().unwrap();
    let sent = mock.requests().pop().unwrap();
    assert_eq!(
        (sent.method.as_str(), sent.url.as_str()),
        ("POST", "/api/auth/logout")
    );
    assert_eq!(
        sent.header("Authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert!(mock.requests().iter().all(|r| r
        .header("User-Agent")
        .is_some_and(|u| u.starts_with("avila-account/"))));
}

#[test]
fn a_revoked_token_says_to_sign_in_again() {
    let mock = signed_in_service();
    let client = Client::with_token(&mock.base, "avl_revoked").unwrap();
    for error in [client.whoami().unwrap_err(), client.logout().unwrap_err()] {
        assert_eq!(error, AccountError::Unauthorized);
        let text = error.to_string();
        assert!(text.contains("sign in again"), "{text}");
        assert!(!text.contains("avl_revoked"));
    }
    // Other statuses carry the service's message.
    let failing = Mock::start(|_| (403, json!({"error": "forbidden here"})));
    let error = Client::with_token(&failing.base, TOKEN)
        .unwrap()
        .whoami()
        .unwrap_err();
    assert_eq!(
        error,
        AccountError::Http {
            status: 403,
            message: "forbidden here".into()
        }
    );
    let dead = Client::with_token("http://127.0.0.1:1", TOKEN).unwrap();
    assert!(matches!(dead.whoami(), Err(AccountError::Network(_))));
}

#[test]
fn credentials_file_round_trip_and_mode() {
    let dir = std::env::temp_dir().join(format!("avila-account-creds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("nested").join("credentials.json");
    assert_eq!(Credentials::load_from(&path).unwrap(), None);
    let credentials = Credentials {
        base_url: "http://127.0.0.1:8787".into(),
        token: TOKEN.into(),
        email: "a@example.org".into(),
        scope: "read".into(),
    };
    credentials.save_to(&path).unwrap();
    assert_eq!(
        Credentials::load_from(&path).unwrap(),
        Some(credentials.clone())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    // Saving over a file keeps the mode and leaves no temp file behind.
    credentials.save_to(&path).unwrap();
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
    assert!(Credentials::clear_at(&path).unwrap());
    assert!(!Credentials::clear_at(&path).unwrap());
    std::fs::write(dir.join("bad.json"), "{").unwrap();
    assert!(Credentials::load_from(&dir.join("bad.json")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
