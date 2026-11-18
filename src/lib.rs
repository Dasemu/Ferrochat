//! Ferrochat: unofficial lightweight native WhatsApp desktop client.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod app;
pub mod auth;
pub mod config;
pub mod error;
pub mod media;
pub mod mock;
pub mod net;
pub mod notify;
pub mod store;
pub mod ui;
