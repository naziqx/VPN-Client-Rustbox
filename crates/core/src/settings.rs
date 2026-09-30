use std::path::{Path, PathBuf};

use anyhow::Context;
use rustbox_platform::Platform;
use serde::{Deserialize, Serialize};

use crate::config::CoreKind;
use crate::model::ProfileId;

/// Latency test kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TestKind {
    /// Real request through the core: shows whether the server works.
    #[default]
    Url,
    /// Only whether the port is open: fast, but proves little.
    Tcp,
}

impl TestKind {
    pub const ALL: [TestKind; 2] = [TestKind::Url, TestKind::Tcp];
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Selected core.
    pub core: CoreKind,
    /// Explicit core executable paths; `None` = auto-detect.
    pub sing_box_path: Option<PathBuf>,
    pub xray_path: Option<PathBuf>,

    /// Local mixed (HTTP + SOCKS5) inbound.
    pub listen: String,
    pub port: u16,
    /// Point the desktop system proxy at the local inbound when connected.
    pub system_proxy: bool,
    /// Route all traffic through a TUN interface (sing-box only).
    pub tun: bool,
    pub tun_stack: String,
    pub tun_mtu: u32,

    /// Send LAN / private addresses directly.
    pub bypass_lan: bool,
    /// Remote DNS (resolved through the proxy): `https://…`, `tls://…`, `tcp://…`, `udp://…` or an IP.
    pub dns_remote: String,
    /// Direct DNS used to resolve proxy server addresses: `local` = system resolver.
    pub dns_direct: String,

    pub log_level: String,

    pub test_url: String,
    pub test_timeout_ms: u64,
    pub test_concurrency: usize,
    /// Test run by the "Проверить" button.
    pub test_kind: TestKind,

    pub subscription_user_agent: String,
    /// Update subscriptions through the running proxy.
    pub subscription_via_proxy: bool,

    /// Last selected profile.
    pub selected: Option<ProfileId>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            core: CoreKind::SingBox,
            sing_box_path: None,
            xray_path: None,
            listen: "127.0.0.1".into(),
            port: 2080,
            system_proxy: false,
            tun: false,
            tun_stack: "mixed".into(),
            tun_mtu: 9000,
            bypass_lan: true,
            dns_remote: "https://1.1.1.1/dns-query".into(),
            dns_direct: "local".into(),
            log_level: "warn".into(),
            test_url: "https://www.gstatic.com/generate_204".into(),
            test_timeout_ms: 5000,
            test_concurrency: 10,
            test_kind: TestKind::default(),
            subscription_user_agent: crate::subscription::DEFAULT_USER_AGENT.into(),
            subscription_via_proxy: false,
            selected: None,
        }
    }
}

impl Settings {
    pub fn path(platform: &dyn Platform) -> PathBuf {
        platform.config_dir().join("settings.json")
    }

    /// Loads settings, falling back to defaults when the file is missing.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                serde_json::from_str(&s).with_context(|| format!("invalid {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        crate::storage::write_atomic(path, &serde_json::to_vec_pretty(self)?)
    }

    pub fn core_path_override(&self, kind: CoreKind) -> Option<&Path> {
        match kind {
            CoreKind::SingBox => self.sing_box_path.as_deref(),
            CoreKind::Xray => self.xray_path.as_deref(),
        }
    }

    /// Resolves the executable for `kind`: explicit path first, then platform lookup.
    pub fn core_executable(
        &self,
        kind: CoreKind,
        platform: &dyn Platform,
    ) -> anyhow::Result<PathBuf> {
        if let Some(path) = self.core_path_override(kind) {
            anyhow::ensure!(path.is_file(), "core not found at {}", path.display());
            return Ok(path.to_path_buf());
        }
        platform.find_executable(kind.executable_name()).with_context(|| {
            format!(
                "{} not found. Install it (e.g. `sudo pacman -S {}`) or set the path in settings",
                kind.display_name(),
                kind.arch_package()
            )
        })
    }
}
