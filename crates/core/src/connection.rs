//! A running proxy session: generated config + core process(es) + system proxy.
//!
//! Layouts:
//! * sing-box: one process (mixed inbound, optional TUN).
//! * Xray: one process (mixed inbound).
//! * Xray + TUN: Xray plus a TUN-only sing-box that forwards into Xray's inbound.
//!
//! When sing-box is selected but cannot run a profile (XHTTP, pinned certificates on
//! servers that only answer uTLS handshakes, ...), Xray is used for that profile.

use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use rustbox_platform::{Platform, TunPrivilege};

use crate::config::{CoreKind, ProxyOptions, tun_front_config};
use crate::model::{Profile, ProfileId};
use crate::process::{self, CoreProcess, LogBuffer};
use crate::settings::Settings;

pub struct Connection {
    /// Backend first, TUN front (if any) last.
    processes: Vec<CoreProcess>,
    pub profile_id: ProfileId,
    /// Core that talks to the server.
    pub core: CoreKind,
    /// A separate sing-box provides TUN for `core`.
    pub tun_front: bool,
    pub system_proxy: bool,
    pub tun: bool,
    pub config_path: PathBuf,
}

impl Connection {
    /// Generates the config(s), validates them, starts the core(s) and (optionally)
    /// sets the system proxy.
    pub fn start(
        platform: &dyn Platform,
        settings: &Settings,
        profile: &Profile,
        logs: LogBuffer,
    ) -> anyhow::Result<Self> {
        let mut profile = profile.clone();
        let core = choose_core(platform, settings, &mut profile, &logs)?;
        let exe = settings.core_executable(core, platform)?;
        let tun_front = settings.tun && !core.supports_tun();

        let mut opts = ProxyOptions::from(settings);
        let own_tun = opts
            .tun
            .as_ref()
            .map(|t| t.interface.clone())
            .unwrap_or_default();
        let sing_box = if settings.tun {
            Some(prepare_tun(platform, settings, &own_tun)?)
        } else {
            None
        };
        if let Some(dev) = platform.foreign_vpn_interface(&own_tun) {
            logs.push(format!(
                "[rustbox] WARN: traffic goes through another VPN ({dev}); the proxy is chained through it"
            ));
        }
        if tun_front {
            // The backend must reach the server without DNS through the TUN (which
            // would need the backend itself): resolve the address up front. Full
            // Xray configs have many servers; the front bypasses the backend
            // process by name, which covers them.
            if !matches!(profile.protocol, crate::model::Protocol::Xray { .. }) {
                pin_server_address(&mut profile)?;
            }
            opts.tun = None;
        }

        let dir = platform.runtime_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let config_path = dir.join(format!("{}.json", core.executable_name()));
        let config = core.build_config(&profile, &opts)?;
        std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)?;
        let env = platform.core_environment(settings.tun && !tun_front);
        process::check_config(core, &exe, &config_path, &env)?;

        logs.push(format!(
            "[rustbox] starting {}{} for '{}' ({})",
            core,
            if tun_front { " + sing-box TUN" } else { "" },
            profile.display_name(),
            profile.address()
        ));
        let mut backend =
            CoreProcess::spawn_with_env(core, &exe, &config_path, Some(logs.clone()), &env)?;
        wait_until_listening(&mut backend, &settings.listen, settings.port, &logs)?;
        let mut processes = vec![backend];

        if tun_front {
            let sb = sing_box
                .as_deref()
                .context("sing-box is required for TUN")?;
            let front = start_tun_front(platform, settings, sb, &exe, &dir, &logs)?;
            processes.push(front);
        }

        let mut system_proxy = false;
        if settings.system_proxy {
            match platform.set_system_proxy(&settings.listen, settings.port) {
                Ok(()) => system_proxy = true,
                Err(e) => logs.push(format!("[rustbox] system proxy: {e:#}")),
            }
        }
        logs.push(format!(
            "[rustbox] listening on {}:{} (mixed HTTP/SOCKS5){}",
            settings.listen,
            settings.port,
            if settings.tun { ", TUN on" } else { "" }
        ));

        Ok(Self {
            processes,
            profile_id: profile.id,
            core,
            tun_front,
            system_proxy,
            tun: settings.tun,
            config_path,
        })
    }

    pub fn is_running(&mut self) -> bool {
        self.processes.iter_mut().all(|p| p.is_running())
    }

    pub fn pid(&self) -> u32 {
        self.processes[0].pid()
    }

    /// Stops the core(s) and restores the system proxy.
    pub fn stop(mut self, platform: &dyn Platform) -> anyhow::Result<()> {
        let proxy_result = if self.system_proxy {
            platform.clear_system_proxy()
        } else {
            Ok(())
        };
        // TUN front first, so traffic is not sent to a dead backend.
        while let Some(mut p) = self.processes.pop() {
            p.stop();
        }
        proxy_result
    }
}

/// Checks that the internet is reachable through the running session: one HTTP
/// request through the local inbound (works in TUN mode too).
pub async fn health_check(settings: &Settings) -> anyhow::Result<Duration> {
    let proxy = format!(
        "socks5h://{}:{}",
        connect_host(&settings.listen),
        settings.port
    );
    let timeout = Duration::from_millis(settings.test_timeout_ms.max(3000));
    crate::latency::url_test_via(&proxy, &settings.test_url, timeout).await
}

/// Picks the core for this profile; prepares sing-box certificate pins.
fn choose_core(
    platform: &dyn Platform,
    settings: &Settings,
    profile: &mut Profile,
    logs: &LogBuffer,
) -> anyhow::Result<CoreKind> {
    let core = settings.core;
    if core != CoreKind::SingBox {
        core.check_profile(profile)?;
        return Ok(core);
    }
    let usable = core.check_profile(profile).and_then(|_| {
        crate::tls_pin::resolve_pubkey_pins(profile, Duration::from_secs(8))
            .context("certificate pin (pcs) check failed")
    });
    let Err(err) = usable else { return Ok(core) };
    let fallback = CoreKind::Xray;
    if fallback.check_profile(profile).is_ok()
        && settings.core_executable(fallback, platform).is_ok()
    {
        logs.push(format!(
            "[rustbox] sing-box cannot use this profile ({err:#}); using {fallback} for it"
        ));
        Ok(fallback)
    } else {
        Err(err)
    }
}

/// Checks TUN prerequisites and returns the sing-box executable that will own the TUN.
fn prepare_tun(
    platform: &dyn Platform,
    settings: &Settings,
    own_tun: &str,
) -> anyhow::Result<PathBuf> {
    let sing_box = settings
        .core_executable(CoreKind::SingBox, platform)
        .context("TUN mode needs sing-box")?;
    match platform.tun_privilege(&sing_box) {
        TunPrivilege::Granted => {}
        TunPrivilege::Missing { hint } => {
            bail!("sing-box has no rights to create TUN. Run: {hint}")
        }
        TunPrivilege::Unsupported => bail!("TUN is not supported on {}", platform.name()),
    }
    if let Some(dev) = platform.foreign_vpn_interface(own_tun) {
        bail!(
            "another VPN is active ({dev}). Two TUN clients at once break routing: \
             disconnect it (e.g. Happ) or use RustBox without TUN"
        );
    }
    // The previous instance may still be tearing its interface down.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while platform.interface_exists(own_tun) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if platform.interface_exists(own_tun) {
        bail!("interface {own_tun} is still in use by another process");
    }
    Ok(sing_box)
}

fn start_tun_front(
    platform: &dyn Platform,
    settings: &Settings,
    sing_box: &Path,
    backend_exe: &Path,
    dir: &Path,
    logs: &LogBuffer,
) -> anyhow::Result<CoreProcess> {
    let backend_name = backend_exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "xray".into());
    let upstream = connect_host(&settings.listen);
    let config = tun_front_config(
        &ProxyOptions::from(settings),
        upstream,
        settings.port,
        &[backend_name],
    )?;
    let path = dir.join("sing-box-tun.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&config)?)?;
    let env = platform.core_environment(true);
    process::check_config(CoreKind::SingBox, sing_box, &path, &env)?;
    let mut front =
        CoreProcess::spawn_with_env(CoreKind::SingBox, sing_box, &path, Some(logs.clone()), &env)?;
    // No port to wait for; catch immediate failures (TUN setup errors).
    std::thread::sleep(Duration::from_millis(700));
    if let Some(status) = front.try_wait() {
        bail!(
            "sing-box TUN exited ({status}): {}",
            logs.tail(5).join("\n")
        );
    }
    Ok(front)
}

/// Replaces a domain server address with its IP, keeping the domain as TLS SNI.
fn pin_server_address(profile: &mut Profile) -> anyhow::Result<()> {
    if profile.server.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let domain = profile.server.clone();
    let addr = (domain.as_str(), profile.port)
        .to_socket_addrs()
        .with_context(|| format!("cannot resolve {domain}"))?
        .min_by_key(|a| a.is_ipv6())
        .with_context(|| format!("no address for {domain}"))?;
    if let Some(tls) = profile.tls.as_mut()
        && tls.sni.is_none()
    {
        tls.sni = Some(domain);
    }
    profile.server = addr.ip().to_string();
    Ok(())
}

fn connect_host(listen: &str) -> &str {
    match listen {
        "0.0.0.0" | "" => "127.0.0.1",
        "::" => "::1",
        other => other,
    }
}

/// Waits until the local inbound accepts connections, failing early if the core exits
/// (port in use, bad TUN setup, ...).
fn wait_until_listening(
    process: &mut CoreProcess,
    listen: &str,
    port: u16,
    logs: &LogBuffer,
) -> anyhow::Result<()> {
    let addr = (connect_host(listen), port)
        .to_socket_addrs()?
        .next()
        .context("invalid listen address")?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = process.try_wait() {
            bail!(
                "{} exited ({status}): {}",
                process.kind,
                logs.tail(5).join("\n")
            );
        }
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            bail!("{} did not open {addr} in time", process.kind);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
