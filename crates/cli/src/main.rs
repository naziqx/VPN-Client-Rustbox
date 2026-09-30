use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand, ValueEnum};
use rustbox_core::connection::Connection;
use rustbox_core::latency::{self, UrlTestOptions};
use rustbox_core::model::Subscription;
use rustbox_core::process::LogBuffer;
use rustbox_core::{CoreKind, GroupId, ProfileId, Settings, Store, link, subscription};
use rustbox_platform::Platform;

#[derive(Parser)]
#[command(name = "rustbox-cli", version, about = "RustBox proxy client (CLI)")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List groups
    Groups,
    /// List profiles
    List {
        #[arg(short, long)]
        group: Option<GroupId>,
    },
    /// Import share links from arguments, a file or stdin ("-")
    Import {
        /// Links, a path to a file, or "-" for stdin
        input: Vec<String>,
        #[arg(short, long, default_value_t = 0)]
        group: GroupId,
    },
    /// Manage subscriptions
    #[command(subcommand)]
    Sub(SubCmd),
    /// Remove profiles
    Remove { ids: Vec<ProfileId> },
    /// Print the share link of a profile
    Export { id: ProfileId },
    /// Show or select the core
    Core { kind: Option<CoreArg> },
    /// Latency test
    Ping {
        /// Profiles to test (default: whole group)
        ids: Vec<ProfileId>,
        #[arg(short, long, default_value_t = 0)]
        group: GroupId,
        /// URL test through the core instead of TCP ping
        #[arg(short, long)]
        url: bool,
    },
    /// Connect with a profile and stay in the foreground until Ctrl+C
    Run {
        id: Option<ProfileId>,
        /// Enable system proxy
        #[arg(long)]
        system_proxy: bool,
        /// Enable TUN mode (sing-box only)
        #[arg(long)]
        tun: bool,
    },
    /// Print the generated core config for a profile
    Config { id: ProfileId },
    /// Show paths and environment info
    Info,
}

#[derive(Subcommand)]
enum SubCmd {
    /// Add a subscription group
    Add { name: String, url: String },
    /// Update one subscription group, or all
    Update { group: Option<GroupId> },
    /// Remove a subscription group with its profiles
    Remove { group: GroupId },
}

#[derive(Clone, Copy, ValueEnum)]
enum CoreArg {
    SingBox,
    Xray,
}

impl From<CoreArg> for CoreKind {
    fn from(c: CoreArg) -> Self {
        match c {
            CoreArg::SingBox => CoreKind::SingBox,
            CoreArg::Xray => CoreKind::Xray,
        }
    }
}

struct Ctx {
    platform: Box<dyn Platform>,
    settings: Settings,
    settings_path: PathBuf,
    store: Store,
}

impl Ctx {
    fn load() -> anyhow::Result<Self> {
        let platform = rustbox_platform::current();
        let settings_path = Settings::path(platform.as_ref());
        let settings = Settings::load(&settings_path)?;
        let store = Store::load(&Store::path(platform.as_ref()))?;
        Ok(Self {
            platform,
            settings,
            settings_path,
            store,
        })
    }

    fn save(&self) -> anyhow::Result<()> {
        self.settings.save(&self.settings_path)?;
        self.store.save()
    }
}

#[tokio::main]
async fn main() {
    if let Err(e) = run(Cli::parse()).await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let mut ctx = Ctx::load()?;
    match cli.command {
        Cmd::Groups => {
            for g in &ctx.store.groups {
                let count = ctx.store.profiles_in(g.id).count();
                match &g.subscription {
                    Some(s) => println!("{:>3}  {} ({count})  {}", g.id, g.name, s.url),
                    None => println!("{:>3}  {} ({count})", g.id, g.name),
                }
            }
        }
        Cmd::List { group } => {
            for p in ctx
                .store
                .profiles
                .iter()
                .filter(|p| group.is_none_or(|g| p.group == g))
            {
                let latency = p
                    .latency
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                let mark = if ctx.settings.selected == Some(p.id) {
                    '*'
                } else {
                    ' '
                };
                println!(
                    "{mark}{:>4}  {:<24} {:<32} {:<30} {latency}",
                    p.id,
                    p.type_label(),
                    p.address(),
                    p.display_name()
                );
            }
        }
        Cmd::Import { input, group } => {
            anyhow::ensure!(ctx.store.group(group).is_some(), "no group {group}");
            let text = read_input(&input)?;
            let (profiles, errors) = match subscription::parse_body(&text) {
                Ok(r) => (r.profiles, r.errors),
                Err(_) => link::parse_many(&text),
            };
            for e in &errors {
                eprintln!("skip: {e}");
            }
            let ids = ctx.store.add_profiles(group, profiles);
            ctx.save()?;
            println!("imported {} profiles", ids.len());
        }
        Cmd::Sub(SubCmd::Add { name, url }) => {
            let id = ctx.store.add_group(
                name,
                Some(Subscription {
                    url,
                    ..Default::default()
                }),
            );
            update_subscription(&mut ctx, id).await?;
            ctx.save()?;
        }
        Cmd::Sub(SubCmd::Update { group }) => {
            let groups: Vec<GroupId> = match group {
                Some(g) => vec![g],
                None => ctx
                    .store
                    .groups
                    .iter()
                    .filter(|g| g.subscription.is_some())
                    .map(|g| g.id)
                    .collect(),
            };
            for g in groups {
                if let Err(e) = update_subscription(&mut ctx, g).await {
                    eprintln!("group {g}: {e:#}");
                }
            }
            ctx.save()?;
        }
        Cmd::Sub(SubCmd::Remove { group }) => {
            anyhow::ensure!(ctx.store.remove_group(group), "cannot remove group {group}");
            ctx.save()?;
        }
        Cmd::Remove { ids } => {
            ctx.store.remove_profiles(&ids);
            ctx.save()?;
        }
        Cmd::Export { id } => {
            let p = ctx.store.profile(id).context("no such profile")?;
            println!("{}", link::to_link(p));
        }
        Cmd::Core { kind } => {
            if let Some(kind) = kind {
                ctx.settings.core = kind.into();
                ctx.save()?;
            }
            for kind in CoreKind::ALL {
                let mark = if kind == ctx.settings.core { '*' } else { ' ' };
                let status = match ctx.settings.core_executable(kind, ctx.platform.as_ref()) {
                    Ok(exe) => format!(
                        "{} ({})",
                        exe.display(),
                        rustbox_core::process::core_version(kind, &exe).unwrap_or_default()
                    ),
                    Err(e) => format!("{e:#}"),
                };
                println!("{mark} {:<9} {status}", kind.display_name());
            }
        }
        Cmd::Ping { ids, group, url } => {
            let profiles: Vec<_> = if ids.is_empty() {
                ctx.store.profiles_in(group).cloned().collect()
            } else {
                ids.iter()
                    .filter_map(|id| ctx.store.profile(*id).cloned())
                    .collect()
            };
            anyhow::ensure!(!profiles.is_empty(), "nothing to test");
            let timeout = Duration::from_millis(ctx.settings.test_timeout_ms);
            // Results are printed as they arrive and collected for saving.
            let names: std::collections::HashMap<ProfileId, String> =
                profiles.iter().map(|p| (p.id, p.display_name())).collect();
            let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let sink: latency::ResultSink = {
                let collected = collected.clone();
                std::sync::Arc::new(move |id, latency: rustbox_core::model::Latency| {
                    let name = names.get(&id).cloned().unwrap_or_default();
                    println!("{id:>4}  {:<12} {name}", latency.to_string());
                    collected.lock().unwrap().push((id, latency));
                })
            };
            if url {
                let core = ctx.settings.core;
                let exe = ctx.settings.core_executable(core, ctx.platform.as_ref())?;
                let fallback = (core == CoreKind::SingBox)
                    .then(|| {
                        ctx.settings
                            .core_executable(CoreKind::Xray, ctx.platform.as_ref())
                            .ok()
                    })
                    .flatten();
                let work_dir = ctx.platform.runtime_dir();
                let env = ctx.platform.core_environment(false);
                let opts = UrlTestOptions {
                    core,
                    exe: &exe,
                    fallback: fallback.as_deref().map(|p| (CoreKind::Xray, p)),
                    work_dir: &work_dir,
                    url: &ctx.settings.test_url,
                    timeout,
                    concurrency: ctx.settings.test_concurrency,
                    env: &env,
                };
                latency::url_test_many(opts, profiles, sink).await;
            } else {
                latency::tcp_ping_many(profiles, timeout, ctx.settings.test_concurrency, sink)
                    .await;
            }
            for (id, latency) in collected.lock().unwrap().drain(..) {
                ctx.store.set_latency(id, latency);
            }
            ctx.save()?;
        }
        Cmd::Run {
            id,
            system_proxy,
            tun,
        } => {
            let id = id
                .or(ctx.settings.selected)
                .context("specify a profile id")?;
            let profile = ctx.store.profile(id).context("no such profile")?.clone();
            ctx.settings.selected = Some(id);
            ctx.settings.save(&ctx.settings_path)?;
            let mut settings = ctx.settings.clone();
            settings.system_proxy |= system_proxy;
            settings.tun |= tun;

            let logs = LogBuffer::default();
            let conn = Connection::start(ctx.platform.as_ref(), &settings, &profile, logs.clone())?;
            println!("connected, pid {} — Ctrl+C to stop", conn.pid());
            match rustbox_core::connection::health_check(&settings).await {
                Ok(d) => println!("internet through proxy: OK ({} ms)", d.as_millis()),
                Err(e) => {
                    println!("internet through proxy: FAILED ({e:#}) — the server does not respond")
                }
            }
            let printer = tokio::spawn(print_logs(logs));
            tokio::signal::ctrl_c().await?;
            printer.abort();
            conn.stop(ctx.platform.as_ref())?;
            println!("stopped");
        }
        Cmd::Config { id } => {
            let p = ctx.store.profile(id).context("no such profile")?;
            let cfg = ctx.settings.core.build_config(p, &(&ctx.settings).into())?;
            println!("{}", serde_json::to_string_pretty(&cfg)?);
        }
        Cmd::Info => {
            let p = ctx.platform.as_ref();
            println!("platform: {}", p.name());
            println!("settings: {}", ctx.settings_path.display());
            println!("profiles: {}", Store::path(p).display());
            println!("runtime:  {}", p.runtime_dir().display());
        }
    }
    Ok(())
}

async fn update_subscription(ctx: &mut Ctx, group: GroupId) -> anyhow::Result<()> {
    let g = ctx.store.group(group).context("no such group")?;
    let sub = g
        .subscription
        .clone()
        .context("group has no subscription")?;
    let configured = sub
        .user_agent
        .clone()
        .unwrap_or_else(|| ctx.settings.subscription_user_agent.clone());
    let xray = ctx
        .settings
        .core_executable(CoreKind::Xray, ctx.platform.as_ref())
        .is_ok();
    let agents = subscription::user_agents(&configured, xray);
    let result = subscription::fetch_any(&sub.url, &agents, None).await?;
    for e in &result.errors {
        eprintln!("skip: {e}");
    }
    let count = ctx.store.replace_group_profiles(group, result.profiles);
    let g = ctx.store.group_mut(group).unwrap();
    let s = g.subscription.as_mut().unwrap();
    s.info = result.info;
    s.last_updated = Some(now());
    println!("{}: {count} profiles", g.name);
    Ok(())
}

async fn print_logs(logs: LogBuffer) {
    let mut printed = 0u64;
    loop {
        let generation = logs.generation();
        if generation != printed {
            let new = (generation - printed) as usize;
            for line in logs.tail(new) {
                println!("{line}");
            }
            printed = generation;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn read_input(input: &[String]) -> anyhow::Result<String> {
    match input {
        [] => bail!("nothing to import"),
        [one] if one == "-" => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            Ok(s)
        }
        [one] if !one.contains("://") => {
            std::fs::read_to_string(one).with_context(|| format!("failed to read {one}"))
        }
        links => Ok(links.join("\n")),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
