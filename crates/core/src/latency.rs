//! Latency tests.
//!
//! * TCP ping – time to open a TCP connection to the server (no core needed).
//! * URL test – real HTTP request through the proxy. All profiles are tested in
//!   one temporary core process: every profile gets its own local SOCKS inbound
//!   routed to its outbound. Full Xray configs carry their own routing and get a
//!   process each.

use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use futures::StreamExt;

use crate::config::CoreKind;
use crate::model::{Latency, Profile, ProfileId};
use crate::process::CoreProcess;

pub async fn tcp_ping(server: &str, port: u16, timeout: Duration) -> anyhow::Result<Duration> {
    let addrs: Vec<SocketAddr> =
        tokio::time::timeout(timeout, tokio::net::lookup_host((server, port)))
            .await
            .context("DNS timeout")?
            .context("DNS failed")?
            .collect();
    let addr = *addrs.first().context("no address")?;
    let start = Instant::now();
    tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr))
        .await
        .context("timeout")?
        .context("connect failed")?;
    Ok(start.elapsed())
}

/// Receives each result as soon as it is known (tests report progressively).
pub type ResultSink = Arc<dyn Fn(ProfileId, Latency) + Send + Sync>;

/// TCP-pings many profiles concurrently.
pub async fn tcp_ping_many(
    profiles: Vec<Profile>,
    timeout: Duration,
    concurrency: usize,
    sink: ResultSink,
) {
    futures::stream::iter(profiles)
        .for_each_concurrent(concurrency.max(1), |p| {
            let sink = sink.clone();
            async move {
                let result = tcp_ping(&p.server, p.port, timeout).await;
                sink(p.id, to_latency(result));
            }
        })
        .await;
}

/// Performs a GET through a SOCKS5 proxy and returns the elapsed time.
pub async fn url_test_via(proxy: &str, url: &str, timeout: Duration) -> anyhow::Result<Duration> {
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(proxy)?)
        .timeout(timeout)
        .build()?;
    let start = Instant::now();
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!(short_error(&e)))?;
    let elapsed = start.elapsed();
    anyhow::ensure!(resp.status().as_u16() < 500, "HTTP {}", resp.status());
    Ok(elapsed)
}

pub struct UrlTestOptions<'a> {
    pub core: CoreKind,
    pub exe: &'a Path,
    /// Core used for profiles the main core cannot handle (Xray for sing-box).
    pub fallback: Option<(CoreKind, &'a Path)>,
    pub work_dir: &'a Path,
    pub url: &'a str,
    pub timeout: Duration,
    pub concurrency: usize,
    /// Environment for the core processes (`Platform::core_environment`).
    pub env: &'a [(String, String)],
}

/// Full Xray configs start a process each; keep that bounded.
const MAX_SINGLE_PROCESSES: usize = 6;

/// URL-tests many profiles, one temporary core instance per core kind.
pub async fn url_test_many(opts: UrlTestOptions<'_>, mut profiles: Vec<Profile>, sink: ResultSink) {
    // Profiles the main core cannot run, with the reason.
    let mut rejected: Vec<(Profile, String)> = Vec::new();
    if opts.core.needs_pubkey_pins() {
        let errors =
            crate::tls_pin::resolve_many(&mut profiles, opts.timeout, opts.concurrency).await;
        let failed: Vec<usize> = errors.iter().map(|(i, _)| *i).collect();
        for (i, e) in errors {
            rejected.push((profiles[i].clone(), format!("pin: {e}")));
        }
        profiles = profiles
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !failed.contains(i))
            .map(|(_, p)| p)
            .collect();
    }
    let mut main = Vec::new();
    for p in profiles {
        match opts.core.check_profile(&p) {
            Ok(()) => main.push(p),
            Err(e) => rejected.push((p, format!("{e:#}"))),
        }
    }

    let mut fallback = Vec::new();
    for (p, reason) in rejected {
        match opts.fallback {
            Some((kind, _)) if kind.check_profile(&p).is_ok() => fallback.push(p),
            _ => sink(p.id, Latency::Error(reason)),
        }
    }

    // Both core instances run at once, so all ports are reserved in one go.
    let mut ports = match free_ports(main.len() + fallback.len()) {
        Ok(p) => p,
        Err(e) => {
            for p in main.iter().chain(&fallback) {
                sink(p.id, Latency::Error(format!("{e:#}")));
            }
            return;
        }
    };
    let fallback_ports = ports.split_off(main.len());
    let main_run = run_url_tests(opts.core, opts.exe, &opts, main, ports, sink.clone());
    let fallback_run = async {
        if let Some((kind, exe)) = opts.fallback {
            run_url_tests(kind, exe, &opts, fallback, fallback_ports, sink.clone()).await;
        }
    };
    futures::join!(main_run, fallback_run);
}

async fn run_url_tests(
    core: CoreKind,
    exe: &Path,
    opts: &UrlTestOptions<'_>,
    profiles: Vec<Profile>,
    ports: Vec<u16>,
    sink: ResultSink,
) {
    let (batch, singles): (Vec<_>, Vec<_>) = profiles
        .into_iter()
        .zip(ports)
        .partition(|(p, _)| CoreKind::batch_testable(p));
    let (batch, ports): (Vec<Profile>, Vec<u16>) = batch.into_iter().unzip();

    let batch_run = async {
        if batch.is_empty() {
            return;
        }
        if let Err(e) = try_run_url_tests(core, exe, opts, &batch, &ports, &sink).await {
            let msg = format!("{e:#}");
            for p in &batch {
                sink(p.id, Latency::Error(msg.clone()));
            }
        }
    };
    // Futures are built by a plain iterator map: a `for_each_concurrent` closure
    // borrowing `opts` makes the future too generic for `Send` callers.
    let singles_run = futures::stream::iter(singles.into_iter().map(|(profile, port)| {
        let sink = sink.clone();
        async move {
            let result = run_single_url_test(core, exe, opts, &profile, port).await;
            sink(profile.id, to_latency(result));
        }
    }))
    .buffer_unordered(opts.concurrency.clamp(1, MAX_SINGLE_PROCESSES))
    .collect::<Vec<()>>();
    futures::join!(batch_run, singles_run);
}

/// URL test of one profile in its own core process.
async fn run_single_url_test(
    core: CoreKind,
    exe: &Path,
    opts: &UrlTestOptions<'_>,
    profile: &Profile,
    port: u16,
) -> anyhow::Result<Duration> {
    let config = core.build_single_test_config(profile, port)?;
    std::fs::create_dir_all(opts.work_dir)?;
    let config_path = opts.work_dir.join(format!(
        "urltest-single-{}-{}-{port}.json",
        core.executable_name(),
        std::process::id(),
    ));
    std::fs::write(&config_path, serde_json::to_vec(&config)?)?;
    let mut process = CoreProcess::spawn_with_env(core, exe, &config_path, None, opts.env)?;
    let ready = wait_for_port(port, Duration::from_secs(5), &mut process).await;
    let _ = std::fs::remove_file(&config_path);
    ready?;
    let proxy = format!("socks5h://127.0.0.1:{port}");
    // `process` is dropped (core stopped) when this returns or is cancelled.
    url_test_via(&proxy, opts.url, opts.timeout).await
}

async fn try_run_url_tests(
    core: CoreKind,
    exe: &Path,
    opts: &UrlTestOptions<'_>,
    profiles: &[Profile],
    ports: &[u16],
    sink: &ResultSink,
) -> anyhow::Result<()> {
    let entries: Vec<(u16, &Profile)> = ports.iter().copied().zip(profiles.iter()).collect();
    let config = core.build_test_config(&entries)?;

    std::fs::create_dir_all(opts.work_dir)?;
    let config_path = opts.work_dir.join(format!(
        "urltest-{}-{}-{}.json",
        core.executable_name(),
        std::process::id(),
        ports[0]
    ));
    std::fs::write(&config_path, serde_json::to_vec(&config)?)?;

    let mut process = CoreProcess::spawn_with_env(core, exe, &config_path, None, opts.env)?;
    let ready = wait_for_port(ports[0], Duration::from_secs(5), &mut process).await;
    let _ = std::fs::remove_file(&config_path);
    ready?;

    let jobs: Vec<(u16, ProfileId)> = entries.iter().map(|(port, p)| (*port, p.id)).collect();
    let (url, timeout) = (opts.url.to_string(), opts.timeout);
    futures::stream::iter(jobs)
        .for_each_concurrent(opts.concurrency.max(1), |(port, id)| {
            let (url, sink) = (url.clone(), sink.clone());
            async move {
                let proxy = format!("socks5h://127.0.0.1:{port}");
                sink(id, to_latency(url_test_via(&proxy, &url, timeout).await));
            }
        })
        .await;
    // Dropping `process` stops the core, also when the test is cancelled.
    drop(process);
    Ok(())
}

async fn wait_for_port(port: u16, timeout: Duration, core: &mut CoreProcess) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return Ok(());
        }
        if let Some(status) = core.try_wait() {
            anyhow::bail!("{} exited during test ({status})", core.kind);
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "{} did not start in time",
            core.kind
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Reserves `n` free local TCP ports.
pub fn free_ports(n: usize) -> anyhow::Result<Vec<u16>> {
    let listeners: Vec<TcpListener> = (0..n)
        .map(|_| TcpListener::bind("127.0.0.1:0"))
        .collect::<Result<_, _>>()?;
    listeners
        .iter()
        .map(|l| Ok(l.local_addr()?.port()))
        .collect()
}

fn to_latency(result: anyhow::Result<Duration>) -> Latency {
    match result {
        Ok(d) => Latency::Ms(d.as_millis().max(1) as u32),
        Err(e) => Latency::Error(format!("{e:#}")),
    }
}

fn short_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".into()
    } else if e.is_connect() {
        "connect failed".into()
    } else {
        let mut msg = e.to_string();
        let mut source = std::error::Error::source(e);
        while let Some(s) = source {
            msg = s.to_string();
            source = s.source();
        }
        msg
    }
}
