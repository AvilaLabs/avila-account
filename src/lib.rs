//! Avila Labs account sign-in and tool launcher for egui apps, in the
//! browser and on the desktop.
//!
//! * feature `ui`: the launcher, the account chip, the brand palette, and
//!   [`ui_web::WebSuite`] (browser: asks the account service who is signed
//!   in; builds on every target, the browser plumbing is wasm32-only).
//! * feature `client`: [`account`], the device-code sign-in client and the
//!   credentials file. With `ui` as well: [`ui_client`] and
//!   [`ui_desktop::DesktopSuite`] (desktop sign-in).
//!
//! Every tool works without an account; signing in is optional.

mod manifest;
pub use manifest::{Field, Manifest, Tool, DEFAULT_MANIFEST, SCHEMA};

#[cfg(feature = "client")]
pub mod account;
#[cfg(feature = "ui")]
pub mod ui;
#[cfg(all(feature = "ui", feature = "client"))]
pub mod ui_client;
#[cfg(all(feature = "ui", feature = "client"))]
pub mod ui_desktop;
#[cfg(feature = "ui")]
pub mod ui_web;
