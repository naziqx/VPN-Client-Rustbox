//! Linux implementation (developed and tested on Arch Linux).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::{Platform, TunPrivilege};

const APP: &str = "rustbox";

pub struct LinuxPlatform {
    desktop: String,
}

impl LinuxPlatform {
    pub fn new() -> Self {
        Self {
            desktop: std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
        }
    }

    fn is_kde(&self) -> bool {
        self.desktop
            .split(':')
            .any(|d| d.eq_ignore_ascii_case("KDE"))
    }

    fn gsettings_set(schema: &str, key: &str, value: &str) -> anyhow::Result<()> {
        run("gsettings", &["set", schema, key, value])
    }

    fn set_gnome_proxy(host: &str, port: u16) -> anyhow::Result<()> {
        let host = format!("'{host}'");
        let port = port.to_string();
        for proto in ["http", "https", "socks"] {
            let schema = format!("org.gnome.system.proxy.{proto}");
            Self::gsettings_set(&schema, "host", &host)?;
            Self::gsettings_set(&schema, "port", &port)?;
        }
        Self::gsettings_set(
            "org.gnome.system.proxy",
            "ignore-hosts",
            "['localhost', '127.0.0.0/8', '::1', '10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16']",
        )?;
        Self::gsettings_set("org.gnome.system.proxy", "mode", "'manual'")
    }

    fn set_kde_proxy(proxy: Option<(&str, u16)>) -> anyhow::Result<()> {
        let kwrite = |key: &str, value: &str| {
            run(
                "kwriteconfig6",
                &[
                    "--file",
                    "kioslaverc",
                    "--group",
                    "Proxy Settings",
                    "--key",
                    key,
                    value,
                ],
            )
        };
        match proxy {
            Some((host, port)) => {
                kwrite("httpProxy", &format!("http://{host} {port}"))?;
                kwrite("httpsProxy", &format!("http://{host} {port}"))?;
                kwrite("socksProxy", &format!("socks://{host} {port}"))?;
                kwrite("NoProxyFor", "localhost,127.0.0.0/8,::1")?;
                kwrite("ProxyType", "1")?;
            }
            None => kwrite("ProxyType", "0")?,
        }
        // Ask running KDE apps to reload proxy settings; failure is not fatal.
        let _ = run(
            "dbus-send",
            &[
                "--type=signal",
                "/KIO/Scheduler",
                "org.kde.KIO.Scheduler.reparseSlaveConfiguration",
                "string:",
            ],
        );
        Ok(())
    }

    fn is_root() -> bool {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("Uid:"))
                    .and_then(|l| l.split_whitespace().nth(2).map(|euid| euid == "0"))
            })
            .unwrap_or(false)
    }
}

impl Default for LinuxPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for LinuxPlatform {
    fn name(&self) -> String {
        if self.desktop.is_empty() {
            "Linux".into()
        } else {
            format!("Linux ({})", self.desktop)
        }
    }

    fn config_dir(&self) -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| home().join(".config"))
            .join(APP)
    }

    fn data_dir(&self) -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| home().join(".local/share"))
            .join(APP)
    }

    fn runtime_dir(&self) -> PathBuf {
        dirs::runtime_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(APP)
    }

    fn find_executable(&self, name: &str) -> Option<PathBuf> {
        crate::search_path(
            name,
            &[
                self.data_dir().join("cores"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/opt").join(name),
            ],
        )
    }

    fn set_system_proxy(&self, host: &str, port: u16) -> anyhow::Result<()> {
        let mut applied = Vec::new();
        let mut errors = Vec::new();
        if has_command("gsettings") {
            match Self::set_gnome_proxy(host, port) {
                Ok(()) => applied.push("gsettings"),
                Err(e) => errors.push(format!("gsettings: {e:#}")),
            }
        }
        if self.is_kde() && has_command("kwriteconfig6") {
            match Self::set_kde_proxy(Some((host, port))) {
                Ok(()) => applied.push("kde"),
                Err(e) => errors.push(format!("kde: {e:#}")),
            }
        }
        if applied.is_empty() {
            bail!(
                "no supported proxy backend found ({}). Set manually: \
                 export http_proxy=http://{host}:{port} https_proxy=http://{host}:{port} all_proxy=socks5://{host}:{port}",
                errors.join("; ")
            );
        }
        tracing::info!("system proxy set via {}", applied.join(", "));
        Ok(())
    }

    fn clear_system_proxy(&self) -> anyhow::Result<()> {
        if has_command("gsettings") {
            Self::gsettings_set("org.gnome.system.proxy", "mode", "'none'")?;
        }
        if self.is_kde() && has_command("kwriteconfig6") {
            Self::set_kde_proxy(None)?;
        }
        Ok(())
    }

    fn tun_privilege(&self, core: &Path) -> TunPrivilege {
        if Self::is_root() {
            return TunPrivilege::Granted;
        }
        let caps = Command::new("getcap")
            .arg(core)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        if caps.contains("cap_net_admin") {
            TunPrivilege::Granted
        } else {
            TunPrivilege::Missing {
                hint: format!(
                    "sudo setcap cap_net_admin,cap_net_bind_service=+ep {} \
                     (нужно повторять после обновления пакета ядра)",
                    core.display()
                ),
            }
        }
    }

    fn grant_tun_privilege(&self, core: &Path) -> anyhow::Result<()> {
        let core = core.to_str().context("non UTF-8 core path")?;
        run(
            "pkexec",
            &["setcap", "cap_net_admin,cap_net_bind_service=+ep", core],
        )
    }

    fn core_environment(&self, tun: bool) -> Vec<(String, String)> {
        // With TUN, sing-box calls `resolvectl` (domain / default-route / dns) for its
        // interface; unprivileged, each call triggers a polkit password prompt. Hiding
        // it from PATH makes sing-box skip that step: DNS still works because all port 53
        // traffic enters the TUN and is hijacked by the core.
        let mut env = Vec::new();
        if tun {
            env.push(("PATH".into(), String::new()));
        }
        // Xray looks for geoip.dat/geosite.dat next to its binary; distro packages
        // install them elsewhere (Arch: v2ray-geoip, v2ray-domain-list-community).
        // Without them any config with geosite:/geoip: rules fails to start.
        if std::env::var_os("XRAY_LOCATION_ASSET").is_none()
            && let Some(dir) = xray_asset_dir()
        {
            env.push(("XRAY_LOCATION_ASSET".into(), dir));
        }
        env
    }

    fn foreign_vpn_interface(&self, own_interface: &str) -> Option<String> {
        let out = Command::new("ip")
            .args(["route", "get", "1.1.1.1"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let dev = text
            .split_whitespace()
            .skip_while(|w| *w != "dev")
            .nth(1)?
            .to_string();
        let is_vpn = ["tun", "wg", "tap", "utun", "tailscale"]
            .iter()
            .any(|p| dev.starts_with(p));
        (is_vpn && dev != own_interface).then_some(dev)
    }

    fn interface_exists(&self, name: &str) -> bool {
        Path::new("/sys/class/net").join(name).exists()
    }
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn has_command(name: &str) -> bool {
    crate::search_path(name, &[]).is_some()
}

fn run(cmd: &str, args: &[&str]) -> anyhow::Result<()> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {cmd}"))?;
    if !out.status.success() {
        bail!(
            "{cmd} {} exited with {}: {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn xray_asset_dir() -> Option<String> {
    [
        "/usr/share/xray",
        "/usr/local/share/xray",
        "/usr/share/v2ray",
        "/usr/local/share/v2ray",
    ]
    .into_iter()
    .find(|d| std::path::Path::new(d).join("geosite.dat").exists())
    .map(String::from)
}
