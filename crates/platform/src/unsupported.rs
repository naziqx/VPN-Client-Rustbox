//! Fallback for operating systems that have no implementation yet.
//! Paths work everywhere via `dirs`; OS integration features report errors.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::bail;

use crate::{Platform, TunPrivilege};

pub struct UnsupportedPlatform;

const APP: &str = "rustbox";

impl Platform for UnsupportedPlatform {
    fn name(&self) -> String {
        std::env::consts::OS.to_string()
    }

    fn config_dir(&self) -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP)
    }

    fn data_dir(&self) -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP)
    }

    fn runtime_dir(&self) -> PathBuf {
        std::env::temp_dir().join(APP)
    }

    fn find_executable(&self, name: &str) -> Option<PathBuf> {
        crate::search_path(name, &[self.data_dir().join("cores")])
    }

    fn set_system_proxy(&self, _host: &str, _port: u16) -> anyhow::Result<()> {
        bail!("system proxy is not implemented for {}", self.name())
    }

    fn clear_system_proxy(&self) -> anyhow::Result<()> {
        bail!("system proxy is not implemented for {}", self.name())
    }

    fn tun_privilege(&self, _core: &Path) -> TunPrivilege {
        TunPrivilege::Unsupported
    }

    fn grant_tun_privilege(&self, _core: &Path) -> anyhow::Result<()> {
        bail!("TUN is not implemented for {}", self.name())
    }
}
