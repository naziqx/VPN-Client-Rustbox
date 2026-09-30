//! Windows implementation.
//!
//! * System proxy: WinINet settings in the registry (HKCU `Internet Settings`),
//!   followed by a WinINet notification so running browsers pick it up. The
//!   user's previous settings are saved and restored on disconnect.
//! * TUN: sing-box creates the wintun adapter itself but needs an elevated
//!   process, so TUN is available when RustBox runs as administrator.
//! * Cores are looked up next to `rustbox.exe`, in `cores\` beside it, in the
//!   data directory and on `PATH`.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::Networking::WinInet::{
    INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED, InternetSetOptionW,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::{Platform, TunPrivilege};

const APP: &str = "rustbox";
const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
/// Addresses that must not go through the proxy.
const PROXY_OVERRIDE: &str = "localhost;127.*;10.*;172.16.*;172.17.*;172.18.*;172.19.*;\
    172.20.*;172.21.*;172.22.*;172.23.*;172.24.*;172.25.*;172.26.*;172.27.*;172.28.*;\
    172.29.*;172.30.*;172.31.*;192.168.*;<local>";

pub struct WindowsPlatform;

impl WindowsPlatform {
    fn exe_dir() -> Option<PathBuf> {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
    }

    fn core_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(d) = Self::exe_dir() {
            dirs.push(d.join("cores"));
            dirs.push(d);
        }
        dirs.push(self.data_dir().join("cores"));
        dirs
    }

    /// Previous WinINet settings, saved before we enable the system proxy.
    fn backup_path(&self) -> PathBuf {
        self.config_dir().join("system-proxy-backup.txt")
    }
}

impl Platform for WindowsPlatform {
    fn name(&self) -> String {
        "Windows".into()
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
        crate::search_path(name, &self.core_dirs())
    }

    fn set_system_proxy(&self, host: &str, port: u16) -> anyhow::Result<()> {
        let host = match host {
            "" | "0.0.0.0" => "127.0.0.1".to_string(),
            "::" => "[::1]".to_string(),
            h if h.contains(':') && !h.starts_with('[') => format!("[{h}]"),
            h => h.to_string(),
        };
        set_proxy(&format!("{host}:{port}"), self)
    }

    fn clear_system_proxy(&self) -> anyhow::Result<()> {
        let key = RegKey::open(INTERNET_SETTINGS, true)?;
        let backup = std::fs::read_to_string(self.backup_path()).ok();
        match backup.as_deref().and_then(ProxyBackup::parse) {
            Some(b) => {
                key.set_dword("ProxyEnable", b.enable)?;
                restore_string(&key, "ProxyServer", b.server.as_deref())?;
                restore_string(&key, "ProxyOverride", b.bypass.as_deref())?;
            }
            None => key.set_dword("ProxyEnable", 0)?,
        }
        let _ = std::fs::remove_file(self.backup_path());
        notify_wininet();
        Ok(())
    }

    fn tun_privilege(&self, _core: &Path) -> TunPrivilege {
        if is_elevated() {
            TunPrivilege::Granted
        } else {
            TunPrivilege::Missing {
                hint: "Закройте RustBox и запустите его от имени администратора: \
                       правый клик по rustbox.exe → «Запуск от имени администратора»"
                    .into(),
            }
        }
    }

    fn grant_tun_privilege(&self, _core: &Path) -> anyhow::Result<()> {
        bail!("на Windows для TUN запустите RustBox от имени администратора")
    }

    fn core_environment(&self, _tun: bool) -> Vec<(String, String)> {
        // Xray looks for geoip.dat/geosite.dat next to xray.exe; point it at
        // the bundled files if they live elsewhere.
        if std::env::var_os("XRAY_LOCATION_ASSET").is_some() {
            return Vec::new();
        }
        self.core_dirs()
            .into_iter()
            .find(|d| d.join("geosite.dat").is_file())
            .map(|d| vec![("XRAY_LOCATION_ASSET".into(), d.display().to_string())])
            .unwrap_or_default()
    }
}

fn set_proxy(server: &str, platform: &WindowsPlatform) -> anyhow::Result<()> {
    let key = RegKey::open(INTERNET_SETTINGS, true)?;
    // Save the user's settings once; a second connect must not overwrite the
    // backup with our own values.
    let backup_path = platform.backup_path();
    if !backup_path.exists() {
        let backup = ProxyBackup {
            enable: key.get_dword("ProxyEnable").unwrap_or(0),
            server: key.get_string("ProxyServer"),
            bypass: key.get_string("ProxyOverride"),
        };
        if let Some(dir) = backup_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&backup_path, backup.to_text())
            .with_context(|| format!("failed to write {}", backup_path.display()))?;
    }
    key.set_string("ProxyServer", server)?;
    key.set_string("ProxyOverride", PROXY_OVERRIDE)?;
    key.set_dword("ProxyEnable", 1)?;
    notify_wininet();
    Ok(())
}

fn restore_string(key: &RegKey, name: &str, value: Option<&str>) -> anyhow::Result<()> {
    match value {
        Some(v) => key.set_string(name, v),
        None => key.delete(name),
    }
}

/// Tells WinINet (and the browsers using it) that proxy settings changed.
fn notify_wininet() {
    // SAFETY: documented calls with no buffers.
    unsafe {
        InternetSetOptionW(
            std::ptr::null(),
            INTERNET_OPTION_SETTINGS_CHANGED,
            std::ptr::null(),
            0,
        );
        InternetSetOptionW(
            std::ptr::null(),
            INTERNET_OPTION_REFRESH,
            std::ptr::null(),
            0,
        );
    }
}

fn is_elevated() -> bool {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo-handle of the current process needs no closing; the
    // token handle is closed below; the output buffer has the expected size.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            (&raw mut elevation).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// The user's proxy settings before we touched them.
struct ProxyBackup {
    enable: u32,
    server: Option<String>,
    bypass: Option<String>,
}

impl ProxyBackup {
    /// Three lines; a missing value is stored as a lone "-" (values never
    /// contain newlines).
    fn to_text(&self) -> String {
        let opt = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
        format!(
            "{}\n{}\n{}\n",
            self.enable,
            opt(&self.server),
            opt(&self.bypass)
        )
    }

    fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let enable = lines.next()?.trim().parse().ok()?;
        let opt = |l: Option<&str>| l.filter(|v| *v != "-").map(String::from);
        Some(Self {
            enable,
            server: opt(lines.next()),
            bypass: opt(lines.next()),
        })
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// An open registry key under HKCU, closed on drop.
struct RegKey(HKEY);

impl RegKey {
    fn open(path: &str, write: bool) -> anyhow::Result<Self> {
        let mut key: HKEY = std::ptr::null_mut();
        let access = if write {
            KEY_READ | KEY_WRITE
        } else {
            KEY_READ
        };
        // SAFETY: valid NUL-terminated path; `key` receives the handle.
        let err =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide(path).as_ptr(), 0, access, &mut key) };
        if err != ERROR_SUCCESS {
            bail!("cannot open HKCU\\{path} (error {err})");
        }
        Ok(Self(key))
    }

    fn get_dword(&self, name: &str) -> Option<u32> {
        let mut value = 0u32;
        let mut size = size_of::<u32>() as u32;
        let mut kind = 0;
        // SAFETY: the buffer is a u32 and `size` says so.
        let err = unsafe {
            RegQueryValueExW(
                self.0,
                wide(name).as_ptr(),
                std::ptr::null(),
                &mut kind,
                (&raw mut value).cast(),
                &mut size,
            )
        };
        (err == ERROR_SUCCESS && kind == REG_DWORD).then_some(value)
    }

    fn get_string(&self, name: &str) -> Option<String> {
        let name = wide(name);
        let mut size = 0u32;
        let mut kind = 0;
        // SAFETY: first call only asks for the size.
        let err = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if err != ERROR_SUCCESS || kind != REG_SZ {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut bytes = (buf.len() * 2) as u32;
        // SAFETY: `buf` holds `bytes` bytes.
        let err = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                buf.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if err != ERROR_SUCCESS {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    fn set_dword(&self, name: &str, value: u32) -> anyhow::Result<()> {
        // SAFETY: 4-byte buffer for REG_DWORD.
        let err = unsafe {
            RegSetValueExW(
                self.0,
                wide(name).as_ptr(),
                0,
                REG_DWORD,
                (&raw const value).cast(),
                4,
            )
        };
        if err != ERROR_SUCCESS {
            bail!("cannot set {name} (error {err})");
        }
        Ok(())
    }

    fn set_string(&self, name: &str, value: &str) -> anyhow::Result<()> {
        let data = wide(value);
        // SAFETY: NUL-terminated UTF-16 buffer; size in bytes including the NUL.
        let err = unsafe {
            RegSetValueExW(
                self.0,
                wide(name).as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if err != ERROR_SUCCESS {
            bail!("cannot set {name} (error {err})");
        }
        Ok(())
    }

    fn delete(&self, name: &str) -> anyhow::Result<()> {
        // SAFETY: valid key and NUL-terminated name.
        let err = unsafe { RegDeleteValueW(self.0, wide(name).as_ptr()) };
        // Already absent is fine.
        if err != ERROR_SUCCESS && err != windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND {
            bail!("cannot delete {name} (error {err})");
        }
        Ok(())
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: the handle came from RegOpenKeyExW.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_roundtrip() {
        let b = ProxyBackup {
            enable: 1,
            server: Some("10.0.0.1:3128".into()),
            bypass: None,
        };
        let back = ProxyBackup::parse(&b.to_text()).unwrap();
        assert_eq!(back.enable, 1);
        assert_eq!(back.server.as_deref(), Some("10.0.0.1:3128"));
        assert_eq!(back.bypass, None);
    }
}
