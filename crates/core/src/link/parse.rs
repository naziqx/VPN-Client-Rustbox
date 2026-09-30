use serde_json::Value;

use super::uri::{RawUri, decode, split_host_port};
use super::{LinkError, decode_base64_str};
use crate::model::{Profile, Protocol, Reality, Tls, Transport, normalize_cert_hash};

pub fn parse_link(link: &str) -> Result<Profile, LinkError> {
    let link = link.trim();
    let scheme = link
        .split_once("://")
        .map(|(s, _)| s.to_ascii_lowercase())
        .ok_or_else(|| LinkError::Malformed("missing ://".into()))?;
    match scheme.as_str() {
        "vmess" => parse_vmess(link),
        "vless" => parse_vless(link),
        "trojan" => parse_trojan(link),
        "ss" => parse_shadowsocks(link),
        "hysteria2" | "hy2" => parse_hysteria2(link),
        "tuic" => parse_tuic(link),
        "socks" | "socks5" => parse_socks(link),
        other => Err(LinkError::UnsupportedScheme(other.to_string())),
    }
}

fn malformed(msg: impl Into<String>) -> LinkError {
    LinkError::Malformed(msg.into())
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn split_list(s: Option<&str>) -> Vec<String> {
    s.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

// ---------------------------------------------------------------- VMess

/// `vmess://base64(json)` (v2rayN format).
fn parse_vmess(link: &str) -> Result<Profile, LinkError> {
    let body = &link["vmess://".len()..];
    let body = body.split('#').next().unwrap_or_default();
    let json = decode_base64_str(body).ok_or_else(|| malformed("vmess: invalid base64"))?;
    let v: Value = serde_json::from_str(&json).map_err(|e| malformed(format!("vmess: {e}")))?;

    // Values may be strings or numbers depending on the generator.
    let get = |key: &str| -> Option<String> {
        match v.get(key)? {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    };

    let server = get("add").ok_or_else(|| malformed("vmess: missing address"))?;
    let port = get("port")
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| malformed("vmess: bad port"))?;
    let uuid = get("id").ok_or_else(|| malformed("vmess: missing id"))?;
    let alter_id = get("aid").and_then(|a| a.parse().ok()).unwrap_or(0);
    let security = get("scy").unwrap_or_else(|| "auto".into());

    let host = get("host");
    let path = get("path").unwrap_or_default();
    let transport = match get("net").as_deref().unwrap_or("tcp") {
        "ws" => Transport::Ws {
            path,
            host: host.clone(),
        },
        "grpc" => Transport::Grpc { service_name: path },
        "h2" | "http" => Transport::Http {
            path,
            host: host.clone(),
        },
        "httpupgrade" => Transport::HttpUpgrade {
            path,
            host: host.clone(),
        },
        "xhttp" | "splithttp" => Transport::Xhttp {
            path,
            host: host.clone(),
            mode: get("mode"),
        },
        _ => Transport::Tcp,
    };

    let tls = (get("tls").as_deref() == Some("tls")).then(|| Tls {
        sni: get("sni").or_else(|| host.clone()),
        alpn: split_list(get("alpn").as_deref()),
        insecure: matches!(get("allowInsecure").as_deref(), Some("1" | "true")),
        fingerprint: get("fp"),
        reality: None,
        ..Default::default()
    });

    let mut p = Profile::new(
        server,
        port,
        Protocol::Vmess {
            uuid,
            alter_id,
            security,
        },
    );
    p.name = get("ps").unwrap_or_default();
    p.tls = tls;
    p.transport = transport;
    Ok(p)
}

// ---------------------------------------------------------------- VLESS / Trojan

/// Transport from standard query parameters (`type`, `path`, `host`, `serviceName`, `mode`).
fn transport_from_query(u: &RawUri) -> Transport {
    let path = u.param("path").unwrap_or_default().to_string();
    let host = non_empty(u.param("host"));
    match u.param("type").unwrap_or("tcp") {
        "ws" => Transport::Ws { path, host },
        "grpc" => Transport::Grpc {
            service_name: u
                .param_any(&["serviceName", "service_name"])
                .unwrap_or(&path)
                .to_string(),
        },
        "http" | "h2" => Transport::Http { path, host },
        "httpupgrade" => Transport::HttpUpgrade { path, host },
        "xhttp" | "splithttp" => Transport::Xhttp {
            path,
            host,
            mode: non_empty(u.param("mode")),
        },
        _ => Transport::Tcp,
    }
}

/// TLS from standard query parameters. `default_tls` is used when `security` is absent.
fn tls_from_query(u: &RawUri, default_tls: bool) -> Option<Tls> {
    let security = u.param("security").map(str::to_ascii_lowercase);
    let enabled = match security.as_deref() {
        Some("tls") | Some("reality") | Some("xtls") => true,
        Some(_) => false,
        None => default_tls,
    };
    if !enabled {
        return None;
    }
    let reality = (security.as_deref() == Some("reality")).then(|| Reality {
        public_key: u
            .param_any(&["pbk", "publicKey"])
            .unwrap_or_default()
            .to_string(),
        short_id: u
            .param_any(&["sid", "shortId"])
            .unwrap_or_default()
            .to_string(),
        spider_x: non_empty(u.param_any(&["spx", "spiderX"])),
    });
    Some(Tls {
        sni: non_empty(u.param_any(&["sni", "peer", "serverName"])),
        alpn: split_list(u.param("alpn")),
        insecure: u.flag(&["allowInsecure", "insecure", "allow_insecure"]),
        fingerprint: non_empty(u.param_any(&["fp", "fingerprint"])),
        reality,
        pinned_cert_sha256: split_list(u.param_any(&["pcs", "pinnedPeerCertSha256"]))
            .iter()
            .map(|h| normalize_cert_hash(h))
            .filter(|h| h.len() == 64)
            .collect(),
        pinned_pubkey_sha256: Vec::new(),
    })
}

fn parse_vless(link: &str) -> Result<Profile, LinkError> {
    let u = RawUri::parse(link)?;
    let uuid = u
        .userinfo_decoded()
        .ok_or_else(|| malformed("vless: missing uuid"))?;
    let mut p = Profile::new(
        u.host.clone(),
        u.first_port()?,
        Protocol::Vless {
            uuid,
            flow: non_empty(u.param("flow")),
        },
    );
    p.name = u.fragment.clone();
    p.tls = tls_from_query(&u, false);
    p.transport = transport_from_query(&u);
    Ok(p)
}

fn parse_trojan(link: &str) -> Result<Profile, LinkError> {
    let u = RawUri::parse(link)?;
    let password = u
        .userinfo_decoded()
        .ok_or_else(|| malformed("trojan: missing password"))?;
    let mut p = Profile::new(
        u.host.clone(),
        u.first_port()?,
        Protocol::Trojan { password },
    );
    p.name = u.fragment.clone();
    p.tls = tls_from_query(&u, true);
    p.transport = transport_from_query(&u);
    Ok(p)
}

// ---------------------------------------------------------------- Shadowsocks

/// Supports SIP002 (`ss://base64(method:pass)@host:port`), plain userinfo
/// (`ss://method:pass@host:port`, used by 2022 ciphers) and the legacy
/// `ss://base64(method:pass@host:port)` form.
fn parse_shadowsocks(link: &str) -> Result<Profile, LinkError> {
    let body = &link["ss://".len()..];
    let (body, name) = match body.split_once('#') {
        Some((b, f)) => (b, decode(f)),
        None => (body, String::new()),
    };
    let (body, query) = match body.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (body, None),
    };
    let body = body.trim_end_matches('/');

    let (userinfo, hostport) = match body.rsplit_once('@') {
        Some((u, h)) => {
            let u = decode(u);
            let userinfo = if u.contains(':') {
                u
            } else {
                decode_base64_str(&u).ok_or_else(|| malformed("ss: invalid base64 userinfo"))?
            };
            (userinfo, h.to_string())
        }
        None => {
            let decoded = decode_base64_str(body).ok_or_else(|| malformed("ss: invalid base64"))?;
            let (u, h) = decoded
                .rsplit_once('@')
                .ok_or_else(|| malformed("ss: missing server"))?;
            (u.to_string(), h.to_string())
        }
    };
    let (method, password) = userinfo
        .split_once(':')
        .ok_or_else(|| malformed("ss: missing method:password"))?;
    let (host, port) = split_host_port(&hostport)?;
    let port: u16 = port.parse().map_err(|_| malformed("ss: bad port"))?;

    let (plugin, plugin_opts) = query
        .and_then(|q| {
            q.split('&')
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| *k == "plugin")
                .map(|(_, v)| decode(v))
        })
        .map(|plugin| match plugin.split_once(';') {
            Some((name, opts)) => (Some(name.to_string()), Some(opts.to_string())),
            None => (Some(plugin), None),
        })
        .unwrap_or((None, None));

    let mut p = Profile::new(
        host,
        port,
        Protocol::Shadowsocks {
            method: method.to_string(),
            password: password.to_string(),
            plugin: plugin.filter(|s| !s.is_empty()),
            plugin_opts,
        },
    );
    p.name = name;
    Ok(p)
}

// ---------------------------------------------------------------- Hysteria2 / TUIC

fn parse_hysteria2(link: &str) -> Result<Profile, LinkError> {
    let u = RawUri::parse(link)?;
    let password = u.userinfo_decoded().unwrap_or_default();
    let obfs_password = match u.param("obfs") {
        Some("salamander") => non_empty(u.param("obfs-password")),
        _ => None,
    };
    let mut p = Profile::new(
        u.host.clone(),
        u.first_port()?,
        Protocol::Hysteria2 {
            password,
            obfs_password,
            up_mbps: u.param("upmbps").and_then(|v| v.parse().ok()),
            down_mbps: u.param("downmbps").and_then(|v| v.parse().ok()),
        },
    );
    p.name = u.fragment.clone();
    p.tls = tls_from_query(&u, true);
    Ok(p)
}

fn parse_tuic(link: &str) -> Result<Profile, LinkError> {
    let u = RawUri::parse(link)?;
    let userinfo = u
        .userinfo
        .clone()
        .ok_or_else(|| malformed("tuic: missing uuid"))?;
    let (uuid, password) = userinfo
        .split_once(':')
        .map(|(a, b)| (decode(a), decode(b)))
        .ok_or_else(|| malformed("tuic: expected uuid:password"))?;
    let mut p = Profile::new(
        u.host.clone(),
        u.first_port()?,
        Protocol::Tuic {
            uuid,
            password,
            congestion_control: u
                .param_any(&["congestion_control", "congestion-control"])
                .unwrap_or("bbr")
                .to_string(),
            udp_relay_mode: u
                .param_any(&["udp_relay_mode", "udp-relay-mode"])
                .unwrap_or("native")
                .to_string(),
        },
    );
    p.name = u.fragment.clone();
    p.tls = tls_from_query(&u, true);
    Ok(p)
}

// ---------------------------------------------------------------- SOCKS

fn parse_socks(link: &str) -> Result<Profile, LinkError> {
    let u = RawUri::parse(link)?;
    let port = u.first_port()?;
    let (username, password) = match u.userinfo.as_deref() {
        None | Some("") => (None, None),
        Some(raw) => {
            let decoded = decode(raw);
            let creds = if decoded.contains(':') {
                decoded
            } else {
                decode_base64_str(&decoded).unwrap_or(decoded)
            };
            match creds.split_once(':') {
                Some((user, pass)) => (Some(user.to_string()), Some(pass.to_string())),
                None => (Some(creds), None),
            }
        }
    };
    let mut p = Profile::new(u.host.clone(), port, Protocol::Socks { username, password });
    p.name = u.fragment.clone();
    Ok(p)
}
