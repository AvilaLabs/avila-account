# avila-account

Avila Labs account sign-in and tool launcher for egui apps, in the browser
and on the desktop. It is what ACTINV, Converra and OpenBNCT show at the
right end of their header: a grid button that opens every Avila Labs tool,
and a "Sign in" button that becomes an account chip once you are signed in.

Signing in is optional. Every tool works fully without an account.

## Features

- `ui`: the launcher, the account chip, the brand palette (`ui::pal`), the
  tool marks, and `ui_web::WebSuite` for the browser build. It builds on
  every target; the browser calls are wasm32 only.
- `client`: `account`, the device-code sign-in client and the credentials
  file. Together with `ui` it adds `ui_client` (the sign-in card) and
  `ui_desktop::DesktopSuite` for the desktop build.

Nothing is on by default. egui 0.36 (0.36.1 or later).

## Browser (wasm) app

```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
avila-account = { version = "0.1", default-features = false, features = ["ui"] }
```

```rust
use avila_account::ui_web::WebSuite;

// once, in the app constructor
let mut suite = WebSuite::new(&cc.egui_ctx, "actinv");

// in the header row, inside a right-to-left layout
suite.controls(ui);            // or suite.header_right(ui) in a plain row

// after the header, once per frame (the first-visit prompt)
suite.prompt(ctx, header_bottom);
```

The page asks `GET {base}/api/auth/me` with the session cookie on start and
when the tab becomes visible again. Any failure reads as signed out.

## Desktop app

```toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
avila-account = { version = "0.1", default-features = false, features = ["ui", "client"] }
```

```rust
use avila_account::ui_desktop::DesktopSuite;

let mut suite = DesktopSuite::new(ctx, "actinv");

suite.controls(ui);                       // header, right to left
suite.show(ctx, header_bottom);           // first-visit prompt and sign-in card
if let Some(notice) = suite.take_notice() {
    // show it in the status line
}
```

"Sign in" opens a device card: a short code and an "Open browser" button.
The token is saved in `credentials.json` (mode 0600) in `AVILA_CONFIG_DIR`,
else `~/.config/avila`. "Sign out" revokes the token on the service and
deletes the file.

The device sign-in names the app as `<tool>-desktop` (`actinv-desktop`,
`converra-desktop`, `openbnct-desktop`).

## Build variable

`AVILA_ACCOUNT_BASE` is read when this crate is compiled
(`option_env!`). Unset, the service is `https://api.avilalabs.org`. Set it to
point a build at another deployment, for example a local test server:

```sh
AVILA_ACCOUNT_BASE=http://127.0.0.1:8820 cargo build --features ui
```

## What is sent to the account service

- Browser build: the session cookie, to `/api/auth/me` and, on "Sign out",
  `/api/auth/logout`. The cookie is set by the service and is not readable by
  the page.
- Desktop build: the device code request (the app name and the scope), the
  device token during sign-in, and afterwards that token as a Bearer header to
  `/api/auth/me` (checked about once a minute) and `/api/auth/logout`. The
  email address is kept in the credentials file for display.

Nothing from the tools' own work is sent: no inputs, results or files.
See https://api.avilalabs.org/privacy.

## Licence

MIT OR Apache-2.0.
