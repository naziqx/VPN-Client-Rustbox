//! Config generation for the supported cores.
//!
//! Adding a new core = add a variant to [`CoreKind`] and a module with
//! `outbound`, `full_config` and `test_config` functions.

mod dns;
mod singbox;
mod xray;

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Profile, Protocol};
use crate::settings::Settings;
use crate::xray_json;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoreKind {
    #[default]
    SingBox,
    Xray,
}

impl CoreKind {
    pub const ALL: [CoreKind; 2] = [CoreKind::SingBox, CoreKind::Xray];

    pub fn display_name(self) -> &'static str {
        match self {
            CoreKind::SingBox => "sing-box",
            CoreKind::Xray => "Xray",
        }
    }

    pub fn executable_name(self) -> &'static str {
        match self {
            CoreKind::SingBox => "sing-box",
            CoreKind::Xray => "xray",
        }
    }

    pub fn arch_package(self) -> &'static str {
        match self {
            CoreKind::SingBox => "sing-box",
            CoreKind::Xray => "xray",
        }
    }

    pub fn supports_tun(self) -> bool {
        matches!(self, CoreKind::SingBox)
    }

    /// Arguments to run the core with a config file.
    pub fn run_args(self, config: &Path) -> Vec<String> {
        let config = config.to_string_lossy().into_owned();
        match self {
            CoreKind::SingBox => vec!["run".into(), "-c".into(), config],
            CoreKind::Xray => vec!["run".into(), "-c".into(), config],
        }
    }

    /// Arguments to validate a config file without running it.
    pub fn check_args(self, config: &Path) -> Vec<String> {
        let config = config.to_string_lossy().into_owned();
        match self {
            CoreKind::SingBox => vec!["check".into(), "-c".into(), config],
            CoreKind::Xray => vec!["run".into(), "-test".into(), "-c".into(), config],
        }
    }

    pub fn version_args(self) -> Vec<String> {
        vec!["version".into()]
    }

    /// Checks that the core can handle `profile`.
    pub fn check_profile(self, profile: &Profile) -> anyhow::Result<()> {
        if let Protocol::Xray { config } = &profile.protocol {
            return match self {
                CoreKind::Xray => xray_json::check(config),
                CoreKind::SingBox => {
                    anyhow::bail!("a full Xray config (subscription in Happ format) needs Xray")
                }
            };
        }
        // Public-key pins are resolved over the network right before start; use a
        // placeholder so the static check does not depend on it.
        let mut profile = profile.clone();
        if let Some(tls) = profile.tls.as_mut()
            && !tls.pinned_cert_sha256.is_empty()
            && tls.pinned_pubkey_sha256.is_empty()
        {
            tls.pinned_pubkey_sha256 = vec!["AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into()];
        }
        self.outbound(&profile, "proxy").map(|_| ())
    }

    /// Whether profiles need `tls_pin::resolve_pubkey_pins` before config generation.
    pub fn needs_pubkey_pins(self) -> bool {
        matches!(self, CoreKind::SingBox)
    }

    pub fn outbound(self, profile: &Profile, tag: &str) -> anyhow::Result<Value> {
        match self {
            CoreKind::SingBox => singbox::outbound(profile, tag),
            CoreKind::Xray => xray::outbound(profile, tag),
        }
    }

    /// Full config for a proxy session.
    pub fn build_config(self, profile: &Profile, opts: &ProxyOptions) -> anyhow::Result<Value> {
        if opts.tun.is_some() && !self.supports_tun() {
            anyhow::bail!("TUN mode is only supported by sing-box");
        }
        if let Protocol::Xray { config } = &profile.protocol {
            self.check_profile(profile)?;
            return xray_json::session_config(
                config,
                &xray_json::Session {
                    inbound: xray_json::mixed_inbound(&opts.listen, opts.port),
                    log_level: &opts.log_level,
                    bypass_lan: opts.bypass_lan,
                },
            );
        }
        match self {
            CoreKind::SingBox => singbox::full_config(profile, opts),
            CoreKind::Xray => xray::full_config(profile, opts),
        }
    }

    /// Whether the profile can share a batch URL-test process with others.
    /// Full Xray configs carry their own routing and run one per process.
    pub fn batch_testable(profile: &Profile) -> bool {
        !matches!(profile.protocol, Protocol::Xray { .. })
    }

    /// Config for batch URL tests: one local SOCKS inbound per profile,
    /// each routed to its own outbound. `entries` = (local port, profile).
    pub fn build_test_config(self, entries: &[(u16, &Profile)]) -> anyhow::Result<Value> {
        match self {
            CoreKind::SingBox => singbox::test_config(entries),
            CoreKind::Xray => xray::test_config(entries),
        }
    }

    /// URL-test config for a profile that runs in its own process
    /// (see [`CoreKind::batch_testable`]): a SOCKS inbound on `port`.
    pub fn build_single_test_config(self, profile: &Profile, port: u16) -> anyhow::Result<Value> {
        let Protocol::Xray { config } = &profile.protocol else {
            return self.build_test_config(&[(port, profile)]);
        };
        self.check_profile(profile)?;
        xray_json::session_config(
            config,
            &xray_json::Session {
                inbound: xray_json::test_inbound(port),
                log_level: "error",
                bypass_lan: false,
            },
        )
    }
}

/// sing-box TUN front for cores without their own TUN (see `singbox::tun_front_config`).
pub fn tun_front_config(
    opts: &ProxyOptions,
    upstream_host: &str,
    upstream_port: u16,
    bypass_processes: &[String],
) -> anyhow::Result<Value> {
    singbox::tun_front_config(opts, upstream_host, upstream_port, bypass_processes)
}

impl std::fmt::Display for CoreKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

#[derive(Debug, Clone)]
pub struct TunOptions {
    pub interface: String,
    pub stack: String,
    pub mtu: u32,
}

/// Inputs of the session config, independent from the storage format.
#[derive(Debug, Clone)]
pub struct ProxyOptions {
    pub listen: String,
    pub port: u16,
    pub tun: Option<TunOptions>,
    pub bypass_lan: bool,
    pub dns_remote: String,
    pub dns_direct: String,
    pub log_level: String,
}

impl From<&Settings> for ProxyOptions {
    fn from(s: &Settings) -> Self {
        Self {
            listen: s.listen.clone(),
            port: s.port,
            tun: s.tun.then(|| TunOptions {
                interface: "rustbox-tun".into(),
                stack: s.tun_stack.clone(),
                mtu: s.tun_mtu,
            }),
            bypass_lan: s.bypass_lan,
            dns_remote: s.dns_remote.clone(),
            dns_direct: s.dns_direct.clone(),
            log_level: s.log_level.clone(),
        }
    }
}

/// Private/LAN ranges routed directly when `bypass_lan` is on.
pub(crate) const PRIVATE_CIDRS: &[&str] = &[
    "0.0.0.0/8",
    "10.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "224.0.0.0/4",
    "::1/128",
    "fc00::/7",
    "fe80::/10",
];

#[cfg(test)]
mod tests;
