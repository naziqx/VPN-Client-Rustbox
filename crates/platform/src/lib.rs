//! OS abstraction layer.
//!
//! Everything that differs between operating systems lives behind the
//! [`Platform`] trait. The rest of the application only talks to
//! `dyn Platform`, so porting RustBox to a new OS means adding one module
//! here and wiring it up in [`current`].

use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(any(target_os = "linux", windows)))]
mod unsupported;
#[cfg(windows)]
mod windows;

/// Whether the core binary is allowed to create a TUN interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunPrivilege {
    /// Core can create TUN devices (root, capabilities, or OS does not need it).
    Granted,
    /// Core lacks the privileges; `hint` describes how to fix it.
    Missing { hint: String },
    /// TUN is not supported on this platform.
    Unsupported,
}

pub trait Platform: Send + Sync {
    /// Human-readable platform name, e.g. "Linux (Hyprland)".
    fn name(&self) -> String;

    /// Directory for user configuration (settings).
    fn config_dir(&self) -> PathBuf;
    /// Directory for persistent data (profiles, groups, rule sets).
    fn data_dir(&self) -> PathBuf;
    /// Directory for temporary runtime files (generated core configs).
    fn runtime_dir(&self) -> PathBuf;

    /// Locate a core executable by name (e.g. "sing-box", "xray").
    fn find_executable(&self, name: &str) -> Option<PathBuf>;

    /// Point the desktop's system proxy to `host:port` (HTTP + SOCKS).
    fn set_system_proxy(&self, host: &str, port: u16) -> anyhow::Result<()>;
    /// Restore the system proxy to "no proxy".
    fn clear_system_proxy(&self) -> anyhow::Result<()>;

    /// Check whether `core` may create TUN interfaces.
    fn tun_privilege(&self, core: &Path) -> TunPrivilege;
    /// Try to grant TUN privileges to `core` (may prompt for a password).
    fn grant_tun_privilege(&self, core: &Path) -> anyhow::Result<()>;

    /// Extra environment for the core process.
    fn core_environment(&self, _tun: bool) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Name of a network interface of *another* VPN that currently carries the
    /// default route (e.g. `tun0` of a different client), if any.
    fn foreign_vpn_interface(&self, _own_interface: &str) -> Option<String> {
        None
    }

    /// Whether a network interface with this name exists.
    fn interface_exists(&self, _name: &str) -> bool {
        false
    }
}

/// Returns the implementation for the OS we were compiled for.
pub fn current() -> Box<dyn Platform> {
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxPlatform::new())
    }
    #[cfg(windows)]
    {
        Box::new(windows::WindowsPlatform)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Box::new(unsupported::UnsupportedPlatform)
    }
}

/// Search `PATH` plus extra directories for an executable.
pub fn search_path(name: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let path_dirs = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    extra
        .iter()
        .cloned()
        .chain(path_dirs)
        .map(|dir| dir.join(&exe))
        .find(|candidate| candidate.is_file())
}
