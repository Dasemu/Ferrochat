//! Ferrochat: unofficial lightweight native WhatsApp desktop client.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod app;
pub mod auth;
pub mod autostart;
pub mod call;
pub mod config;
pub mod error;
pub mod ext;
pub mod icon;
pub mod instance;
pub mod media;
pub mod mock;
pub mod net;
pub mod notify;
pub mod settings;
pub mod store;
pub mod tray;
pub mod ui;
pub mod update;
