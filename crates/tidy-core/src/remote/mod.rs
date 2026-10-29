//! (M16, SPEC §14.13) Tidy on a NAS: the headless agent (`tidy serve`,
//! [`server`]), the PC app's client for it ([`client`]), the JSON both
//! speak ([`api`]) and NAS ↔ PC path mapping ([`map`]).

pub mod api;
pub mod client;
pub mod map;
pub mod server;

pub use client::{normalize_url, NasClient, RemoteError};
