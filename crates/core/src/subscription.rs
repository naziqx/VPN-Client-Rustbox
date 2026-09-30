//! Subscription download and decoding.
//!
//! Supported body formats:
//! * base64-encoded list of share links (v2rayN style)
//! * plain list of share links
//! * Clash / Clash.Meta YAML (`proxies:` section)
//! * JSON array of full Xray configs (what panels give Happ/Streisand), see
//!   [`crate::xray_json`]

use std::time::Duration;

use anyhow::{Context, bail};
use serde_yaml::Value as Yaml;

use crate::link::{decode_base64_str, parse_many};
use crate::model::{Profile, Protocol, Reality, SubscriptionInfo, Tls, Transport};

/// Share links in v2rayN format: understood by every panel.
pub const LINKS_USER_AGENT: &str = "v2rayN/7.0 (RustBox)";
/// Panels (Remnawave, Marzban, …) give full Xray JSON configs — with balancers
/// over several servers — to Happ and Streisand; they match the name in the
/// User-Agent. As share links those entries degrade to one (fallback) server.
pub const XRAY_JSON_USER_AGENT: &str =
    concat!("RustBox/", env!("CARGO_PKG_VERSION"), " (Happ compatible)");
/// Settings value meaning "choose automatically", see [`user_agents`].
pub const DEFAULT_USER_AGENT: &str = "";

/// User-Agents to try, in order. An explicit value is used as is. Automatic mode
/// asks for full Xray configs when Xray is installed and falls back to share
/// links (some panels give Happ encrypted links we cannot read). The old default
/// (`LINKS_USER_AGENT`, stored in existing settings) also counts as automatic.
pub fn user_agents(configured: &str, xray_available: bool) -> Vec<String> {
    let configured = configured.trim();
    if !configured.is_empty() && configured != LINKS_USER_AGENT {
        return vec![configured.to_string()];
    }
    if xray_available {
        vec![XRAY_JSON_USER_AGENT.into(), LINKS_USER_AGENT.into()]
    } else {
        vec![LINKS_USER_AGENT.into()]
    }
}

/// [`fetch`] with each User-Agent in turn until one gives usable profiles.
pub async fn fetch_any(
    url: &str,
    user_agents: &[String],
    proxy: Option<&str>,
) -> anyhow::Result<FetchResult> {
    let mut last_error = None;
    for ua in user_agents {
        match fetch(url, ua, proxy).await {
            Ok(r) => return Ok(r),
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no User-Agent to fetch with")))
}

#[derive(Debug, Default)]
pub struct FetchResult {
    pub profiles: Vec<Profile>,
    /// Lines/entries that could not be parsed.
    pub errors: Vec<String>,
    pub info: Option<SubscriptionInfo>,
}

/// Downloads and decodes a subscription.
/// `proxy` is an optional proxy URL (e.g. `socks5h://127.0.0.1:2080`) to fetch through.
pub async fn fetch(
    url: &str,
    user_agent: &str,
    proxy: Option<&str>,
) -> anyhow::Result<FetchResult> {
    let mut builder = reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(Duration::from_secs(30));
    if let Some(proxy) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }
    let resp = builder
        .build()?
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to fetch {url}"))?;
    if !resp.status().is_success() {
        bail!("subscription server returned {}", resp.status());
    }
    let info = resp
        .headers()
        .get("subscription-userinfo")
        .and_then(|v| v.to_str().ok())
        .map(parse_userinfo);
    let body = resp
        .text()
        .await
        .context("failed to read subscription body")?;
    let mut result = parse_body(&body)?;
    result.info = info;
    Ok(result)
}

/// Decodes a subscription body in any supported format.
pub fn parse_body(body: &str) -> anyhow::Result<FetchResult> {
    let body = body.trim_start_matches('\u{feff}').trim();
    if body.is_empty() {
        bail!("subscription is empty");
    }
    let (profiles, errors) = if let Some(parsed) = crate::xray_json::parse_configs(body) {
        parsed
    } else if looks_like_clash(body) {
        parse_clash(body)?
    } else if body.contains("://") {
        parse_many(body)
    } else {
        let decoded = decode_base64_str(body).context("unknown subscription format")?;
        parse_many(&decoded)
    };
    if profiles.is_empty() {
        bail!(
            "no supported profiles in subscription ({} errors)",
            errors.len()
        );
    }
    Ok(FetchResult {
        profiles,
        errors,
        info: None,
    })
}

/// Parses `upload=1; download=2; total=3; expire=4`.
pub fn parse_userinfo(header: &str) -> SubscriptionInfo {
    let mut info = SubscriptionInfo::default();
    for part in header.split(';') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let v: u64 = v.trim().parse::<f64>().map(|f| f as u64).unwrap_or(0);
        match k.trim() {
            "upload" => info.upload = v,
            "download" => info.download = v,
            "total" => info.total = v,
            "expire" => info.expire = v,
            _ => {}
        }
    }
    info
}

fn looks_like_clash(body: &str) -> bool {
    body.lines().any(|l| l.trim_start().starts_with("proxies:"))
}

// ---------------------------------------------------------------- Clash YAML

fn parse_clash(body: &str) -> anyhow::Result<(Vec<Profile>, Vec<String>)> {
    let doc: Yaml = serde_yaml::from_str(body).context("invalid Clash YAML")?;
    let proxies = doc
        .get("proxies")
        .and_then(Yaml::as_sequence)
        .context("Clash YAML has no proxies list")?;
    let mut profiles = Vec::new();
    let mut errors = Vec::new();
    for proxy in proxies {
        match clash_proxy(proxy) {
            Ok(p) => profiles.push(p),
            Err(e) => errors.push(format!(
                "{}: {e}",
                proxy.get("name").and_then(Yaml::as_str).unwrap_or("?")
            )),
        }
    }
    Ok((profiles, errors))
}

fn y_str(v: &Yaml, key: &str) -> Option<String> {
    match v.get(key)? {
        Yaml::String(s) if !s.is_empty() => Some(s.clone()),
        Yaml::Number(n) => Some(n.to_string()),
        Yaml::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn y_bool(v: &Yaml, key: &str) -> bool {
    match v.get(key) {
        Some(Yaml::Bool(b)) => *b,
        Some(Yaml::String(s)) => s == "true" || s == "1",
        _ => false,
    }
}

fn y_list(v: &Yaml, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Yaml::Sequence(seq)) => seq
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        Some(Yaml::String(s)) => s.split(',').map(|x| x.trim().to_string()).collect(),
        _ => Vec::new(),
    }
}

fn clash_tls(v: &Yaml, force: bool) -> Option<Tls> {
    if !force && !y_bool(v, "tls") {
        return None;
    }
    let reality = v.get("reality-opts").map(|r| Reality {
        public_key: y_str(r, "public-key").unwrap_or_default(),
        short_id: y_str(r, "short-id").unwrap_or_default(),
        spider_x: None,
    });
    Some(Tls {
        sni: y_str(v, "servername").or_else(|| y_str(v, "sni")),
        alpn: y_list(v, "alpn"),
        insecure: y_bool(v, "skip-cert-verify"),
        fingerprint: y_str(v, "client-fingerprint"),
        reality,
        ..Default::default()
    })
}

fn clash_transport(v: &Yaml) -> Transport {
    let network = y_str(v, "network").unwrap_or_else(|| "tcp".into());
    match network.as_str() {
        "ws" => {
            let opts = v.get("ws-opts");
            Transport::Ws {
                path: opts
                    .and_then(|o| y_str(o, "path"))
                    .unwrap_or_else(|| "/".into()),
                host: opts
                    .and_then(|o| o.get("headers"))
                    .and_then(|h| y_str(h, "Host").or_else(|| y_str(h, "host"))),
            }
        }
        "grpc" => Transport::Grpc {
            service_name: v
                .get("grpc-opts")
                .and_then(|o| y_str(o, "grpc-service-name"))
                .unwrap_or_default(),
        },
        "h2" => {
            let opts = v.get("h2-opts");
            Transport::Http {
                path: opts
                    .and_then(|o| y_str(o, "path"))
                    .unwrap_or_else(|| "/".into()),
                host: opts
                    .map(|o| y_list(o, "host"))
                    .and_then(|h| h.into_iter().next()),
            }
        }
        _ => Transport::Tcp,
    }
}

fn clash_proxy(v: &Yaml) -> anyhow::Result<Profile> {
    let kind = y_str(v, "type").context("missing type")?;
    let server = y_str(v, "server").context("missing server")?;
    let port: u16 = y_str(v, "port")
        .and_then(|p| p.parse().ok())
        .context("bad port")?;
    let need = |key: &str| y_str(v, key).with_context(|| format!("missing {key}"));

    let (protocol, tls, transport) = match kind.as_str() {
        "ss" => {
            let (plugin, plugin_opts) = match y_str(v, "plugin").as_deref() {
                Some("obfs") => {
                    let opts = v.get("plugin-opts");
                    let mode = opts
                        .and_then(|o| y_str(o, "mode"))
                        .unwrap_or_else(|| "http".into());
                    let host = opts.and_then(|o| y_str(o, "host"));
                    let mut s = format!("obfs={mode}");
                    if let Some(h) = host {
                        s.push_str(&format!(";obfs-host={h}"));
                    }
                    (Some("obfs-local".to_string()), Some(s))
                }
                Some(other) => (Some(other.to_string()), None),
                None => (None, None),
            };
            (
                Protocol::Shadowsocks {
                    method: need("cipher")?,
                    password: need("password")?,
                    plugin,
                    plugin_opts,
                },
                None,
                Transport::Tcp,
            )
        }
        "vmess" => (
            Protocol::Vmess {
                uuid: need("uuid")?,
                alter_id: y_str(v, "alterId")
                    .and_then(|a| a.parse().ok())
                    .unwrap_or(0),
                security: y_str(v, "cipher").unwrap_or_else(|| "auto".into()),
            },
            clash_tls(v, false),
            clash_transport(v),
        ),
        "vless" => (
            Protocol::Vless {
                uuid: need("uuid")?,
                flow: y_str(v, "flow"),
            },
            clash_tls(v, false),
            clash_transport(v),
        ),
        "trojan" => (
            Protocol::Trojan {
                password: need("password")?,
            },
            clash_tls(v, true),
            clash_transport(v),
        ),
        "hysteria2" => (
            Protocol::Hysteria2 {
                password: y_str(v, "password").unwrap_or_default(),
                obfs_password: y_str(v, "obfs-password"),
                up_mbps: y_str(v, "up").and_then(|s| s.split_whitespace().next()?.parse().ok()),
                down_mbps: y_str(v, "down").and_then(|s| s.split_whitespace().next()?.parse().ok()),
            },
            clash_tls(v, true),
            Transport::Tcp,
        ),
        "tuic" => (
            Protocol::Tuic {
                uuid: need("uuid")?,
                password: need("password")?,
                congestion_control: y_str(v, "congestion-controller")
                    .unwrap_or_else(|| "bbr".into()),
                udp_relay_mode: y_str(v, "udp-relay-mode").unwrap_or_else(|| "native".into()),
            },
            clash_tls(v, true),
            Transport::Tcp,
        ),
        "socks5" => (
            Protocol::Socks {
                username: y_str(v, "username"),
                password: y_str(v, "password"),
            },
            None,
            Transport::Tcp,
        ),
        "http" => (
            Protocol::Http {
                username: y_str(v, "username"),
                password: y_str(v, "password"),
            },
            clash_tls(v, false),
            Transport::Tcp,
        ),
        other => bail!("unsupported Clash proxy type '{other}'"),
    };

    let mut p = Profile::new(server, port, protocol);
    p.name = y_str(v, "name").unwrap_or_default();
    p.tls = tls;
    p.transport = transport;
    Ok(p)
}

#[cfg(test)]
mod user_agent_tests {
    use super::*;

    #[test]
    fn automatic_user_agent() {
        // Xray installed: full configs first, share links as a fallback.
        assert_eq!(
            user_agents("", true),
            vec![
                XRAY_JSON_USER_AGENT.to_string(),
                LINKS_USER_AGENT.to_string()
            ]
        );
        assert_eq!(user_agents("", false), vec![LINKS_USER_AGENT.to_string()]);
        // The old stored default means "automatic" too.
        assert_eq!(user_agents(LINKS_USER_AGENT, true).len(), 2);
        // An explicit value is respected as is.
        assert_eq!(
            user_agents(" clash-verge/v2 ", true),
            vec!["clash-verge/v2".to_string()]
        );
        // Panels match "Happ" case-sensitively.
        assert!(XRAY_JSON_USER_AGENT.contains("Happ"));
    }

    #[test]
    fn json_subscription_body() {
        let body = r#"[{"remarks":"A","inbounds":[],"outbounds":[{"tag":"proxy","protocol":"trojan","settings":{"servers":[{"address":"a.example","port":443,"password":"p"}]}}],"routing":{"rules":[]}}]"#;
        let r = parse_body(body).unwrap();
        assert_eq!(r.profiles.len(), 1);
        assert_eq!(r.profiles[0].name, "A");
        assert_eq!(r.profiles[0].address(), "a.example:443");
        assert_eq!(r.profiles[0].type_label(), "Trojan");
    }
}
