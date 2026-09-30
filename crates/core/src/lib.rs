//! RustBox core: OS-independent logic of the proxy client.
//!
//! * [`model`] – profiles, groups, protocol/TLS/transport descriptions
//! * [`link`] – share-link parsing and export (vmess://, vless://, ...)
//! * [`subscription`] – fetching and decoding subscriptions (base64, plain, Clash YAML)
//! * [`config`] – config generators for the supported cores (sing-box, Xray)
//! * [`process`] – running a core as a child process and collecting its logs
//! * [`latency`] – TCP ping and URL tests
//! * [`tls_pin`] – certificate pinning (`pcs`) support
//! * [`xray_json`] – whole Xray JSON configs from subscriptions (Happ format)
//! * [`storage`] / [`settings`] – persistent state
//! * [`connection`] – glue that starts/stops a proxy session

pub mod config;
pub mod connection;
pub mod latency;
pub mod link;
pub mod model;
pub mod process;
pub mod settings;
pub mod storage;
pub mod subscription;
pub mod tls_pin;
pub mod xray_json;

pub use config::CoreKind;
pub use model::{Group, GroupId, Profile, ProfileId};
pub use settings::Settings;
pub use storage::Store;
