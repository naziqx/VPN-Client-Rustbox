//! sing-box (1.12+) config generator. Uses the new DNS server format and
//! route rule actions (`sniff`, `hijack-dns`), no legacy fields.

use anyhow::{Context, bail};
use serde_json::{Map, Value, json};

use super::ProxyOptions;
use super::dns::DnsServer;
use crate::model::{Profile, Protocol, Tls, Transport};

const TUN_TABLE_INDEX: u32 = 2081;
const TUN_RULE_INDEX: u32 = 9100;

fn log_level(level: &str) -> &str {
    match level {
        "debug" | "info" | "warn" | "error" | "trace" | "fatal" => level,
        "warning" => "warn",
        _ => "warn",
    }
}

fn tls(tls: &Tls, alpn_default: &[&str]) -> anyhow::Result<Value> {
    let mut v = json!({ "enabled": true });
    if let Some(sni) = &tls.sni {
        v["server_name"] = json!(sni);
    }
    if tls.insecure {
        v["insecure"] = json!(true);
    }
    if !tls.alpn.is_empty() {
        v["alpn"] = json!(tls.alpn);
    } else if !alpn_default.is_empty() {
        v["alpn"] = json!(alpn_default);
    }
    // REALITY requires uTLS in sing-box.
    let fingerprint = tls
        .fingerprint
        .clone()
        .or_else(|| tls.reality.as_ref().map(|_| "chrome".to_string()));
    if let Some(fp) = fingerprint {
        v["utls"] = json!({ "enabled": true, "fingerprint": fp });
    }
    if let Some(r) = &tls.reality {
        v["reality"] =
            json!({ "enabled": true, "public_key": r.public_key, "short_id": r.short_id });
    }
    if !tls.pinned_cert_sha256.is_empty() {
        if tls.pinned_pubkey_sha256.is_empty() {
            bail!("certificate pin (pcs) was not resolved; see tls_pin::resolve_pubkey_pins");
        }
        v["certificate_public_key_sha256"] = json!(tls.pinned_pubkey_sha256);
    }
    Ok(v)
}

/// Splits `/path?ed=2048` into the path and the early-data size.
fn ws_early_data(path: &str) -> (String, Option<u32>) {
    let Some((base, query)) = path.split_once('?') else {
        return (path.to_string(), None);
    };
    let mut ed = None;
    let rest: Vec<&str> = query
        .split('&')
        .filter(|kv| match kv.strip_prefix("ed=") {
            Some(v) => {
                ed = v.parse().ok();
                false
            }
            None => true,
        })
        .collect();
    let path = if rest.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", rest.join("&"))
    };
    (path, ed)
}

fn transport(t: &Transport) -> anyhow::Result<Option<Value>> {
    Ok(Some(match t {
        Transport::Tcp => return Ok(None),
        Transport::Ws { path, host } => {
            let (path, ed) = ws_early_data(path);
            let mut v =
                json!({ "type": "ws", "path": if path.is_empty() { "/".into() } else { path } });
            if let Some(h) = host {
                v["headers"] = json!({ "Host": h });
            }
            if let Some(ed) = ed {
                v["max_early_data"] = json!(ed);
                v["early_data_header_name"] = json!("Sec-WebSocket-Protocol");
            }
            v
        }
        Transport::Grpc { service_name } => json!({ "type": "grpc", "service_name": service_name }),
        Transport::Http { path, host } => {
            let mut v = json!({ "type": "http", "path": path });
            if let Some(h) = host {
                v["host"] = json!([h]);
            }
            v
        }
        Transport::HttpUpgrade { path, host } => {
            let mut v = json!({ "type": "httpupgrade", "path": path });
            if let Some(h) = host {
                v["host"] = json!(h);
            }
            v
        }
        Transport::Xhttp { .. } => bail!("XHTTP transport is not supported by sing-box, use Xray"),
    }))
}

pub fn outbound(p: &Profile, tag: &str) -> anyhow::Result<Value> {
    let mut o = Map::new();
    o.insert("tag".into(), json!(tag));
    o.insert("server".into(), json!(p.server));
    o.insert("server_port".into(), json!(p.port));

    let default_tls = Tls::default();
    match &p.protocol {
        Protocol::Vmess {
            uuid,
            alter_id,
            security,
        } => {
            o.insert("type".into(), json!("vmess"));
            o.insert("uuid".into(), json!(uuid));
            o.insert("security".into(), json!(security));
            o.insert("alter_id".into(), json!(alter_id));
        }
        Protocol::Vless { uuid, flow } => {
            o.insert("type".into(), json!("vless"));
            o.insert("uuid".into(), json!(uuid));
            if let Some(flow) = flow {
                o.insert("flow".into(), json!(flow));
            }
            o.insert("packet_encoding".into(), json!("xudp"));
        }
        Protocol::Trojan { password } => {
            o.insert("type".into(), json!("trojan"));
            o.insert("password".into(), json!(password));
        }
        Protocol::Shadowsocks {
            method,
            password,
            plugin,
            plugin_opts,
        } => {
            o.insert("type".into(), json!("shadowsocks"));
            o.insert("method".into(), json!(method));
            o.insert("password".into(), json!(password));
            if let Some(plugin) = plugin {
                let plugin = match plugin.as_str() {
                    "simple-obfs" | "obfs" => "obfs-local",
                    other => other,
                };
                if !matches!(plugin, "obfs-local" | "v2ray-plugin") {
                    bail!("shadowsocks plugin '{plugin}' is not supported by sing-box");
                }
                o.insert("plugin".into(), json!(plugin));
                o.insert(
                    "plugin_opts".into(),
                    json!(plugin_opts.clone().unwrap_or_default()),
                );
            }
        }
        Protocol::Hysteria2 {
            password,
            obfs_password,
            up_mbps,
            down_mbps,
        } => {
            o.insert("type".into(), json!("hysteria2"));
            o.insert("password".into(), json!(password));
            if let Some(obfs) = obfs_password {
                o.insert(
                    "obfs".into(),
                    json!({ "type": "salamander", "password": obfs }),
                );
            }
            if let Some(up) = up_mbps {
                o.insert("up_mbps".into(), json!(up));
            }
            if let Some(down) = down_mbps {
                o.insert("down_mbps".into(), json!(down));
            }
            let t = p.tls.as_ref().unwrap_or(&default_tls);
            o.insert("tls".into(), tls(t, &["h3"])?);
        }
        Protocol::Tuic {
            uuid,
            password,
            congestion_control,
            udp_relay_mode,
        } => {
            o.insert("type".into(), json!("tuic"));
            o.insert("uuid".into(), json!(uuid));
            o.insert("password".into(), json!(password));
            o.insert("congestion_control".into(), json!(congestion_control));
            o.insert("udp_relay_mode".into(), json!(udp_relay_mode));
            let t = p.tls.as_ref().unwrap_or(&default_tls);
            o.insert("tls".into(), tls(t, &["h3"])?);
        }
        Protocol::Socks { username, password } => {
            o.insert("type".into(), json!("socks"));
            o.insert("version".into(), json!("5"));
            if let Some(u) = username {
                o.insert("username".into(), json!(u));
                o.insert(
                    "password".into(),
                    json!(password.clone().unwrap_or_default()),
                );
            }
        }
        Protocol::Http { username, password } => {
            o.insert("type".into(), json!("http"));
            if let Some(u) = username {
                o.insert("username".into(), json!(u));
                o.insert(
                    "password".into(),
                    json!(password.clone().unwrap_or_default()),
                );
            }
        }
        Protocol::Xray { .. } => {
            bail!("a full Xray config (subscription in Happ format) needs Xray")
        }
    }

    if !matches!(
        p.protocol,
        Protocol::Hysteria2 { .. } | Protocol::Tuic { .. }
    ) {
        if let Some(t) = &p.tls {
            o.insert("tls".into(), tls(t, &[])?);
        }
        if let Some(t) = transport(&p.transport)? {
            o.insert("transport".into(), t);
        }
    } else if !p.transport.is_tcp() {
        bail!("{} does not use a transport", p.protocol.name());
    }
    Ok(Value::Object(o))
}

fn dns_server(spec: &str, tag: &str, detour: Option<&str>) -> anyhow::Result<Value> {
    let d = DnsServer::parse(spec)?;
    let mut v = json!({ "type": d.kind, "tag": tag });
    if d.kind == "local" {
        return Ok(v);
    }
    v["server"] = json!(d.server);
    if let Some(port) = d.port {
        v["server_port"] = json!(port);
    }
    if let Some(path) = &d.path {
        v["path"] = json!(path);
    }
    if let Some(detour) = detour {
        v["detour"] = json!(detour);
    }
    if !d.server_is_ip() {
        v["domain_resolver"] = json!("local");
    }
    Ok(v)
}

pub fn full_config(p: &Profile, opts: &ProxyOptions) -> anyhow::Result<Value> {
    let mut inbounds = vec![json!({
        "type": "mixed",
        "tag": "mixed-in",
        "listen": opts.listen,
        "listen_port": opts.port,
    })];
    if let Some(tun) = &opts.tun {
        inbounds.push(json!({
            "type": "tun",
            "tag": "tun-in",
            "interface_name": tun.interface,
            "address": ["172.19.0.1/30", "fdfe:dcba:9876::1/126"],
            "mtu": tun.mtu,
            "auto_route": true,
            "strict_route": true,
            "stack": tun.stack,
            // Defaults (2022 / 9000) are shared by every sing-box based client; using
            // our own values avoids wiping another client's routes on start/stop.
            "iproute2_table_index": TUN_TABLE_INDEX,
            "iproute2_rule_index": TUN_RULE_INDEX,
        }));
    }

    let mut dns_servers = vec![dns_server(&opts.dns_remote, "remote", Some("proxy"))?];
    let direct = DnsServer::parse(&opts.dns_direct)?;
    if direct.kind == "local" {
        dns_servers.push(json!({ "type": "local", "tag": "local" }));
    } else {
        anyhow::ensure!(
            direct.server_is_ip(),
            "direct DNS must be an IP address or 'local'"
        );
        dns_servers.push(dns_server(&opts.dns_direct, "local", None)?);
    }

    let mut rules = vec![
        json!({ "action": "sniff" }),
        json!({ "protocol": "dns", "action": "hijack-dns" }),
    ];
    if opts.bypass_lan {
        rules.push(json!({ "ip_is_private": true, "outbound": "direct" }));
    }

    let mut dns_rules = Vec::new();
    // Resolve the proxy server itself directly to avoid a chicken-and-egg loop.
    if p.server.parse::<std::net::IpAddr>().is_err() {
        dns_rules.push(json!({ "domain": [p.server], "server": "local" }));
    }

    Ok(json!({
        "log": { "level": log_level(&opts.log_level), "timestamp": true },
        "dns": {
            "servers": dns_servers,
            "rules": dns_rules,
            "final": "remote",
            "strategy": "prefer_ipv4",
        },
        "inbounds": inbounds,
        "outbounds": [
            outbound(p, "proxy")?,
            { "type": "direct", "tag": "direct" },
        ],
        "route": {
            "rules": rules,
            "final": "proxy",
            "auto_detect_interface": true,
            "default_domain_resolver": "local",
        },
    }))
}

pub fn test_config(entries: &[(u16, &Profile)]) -> anyhow::Result<Value> {
    let mut inbounds = Vec::new();
    let mut outbounds = Vec::new();
    let mut rules = Vec::new();
    for (i, (port, profile)) in entries.iter().enumerate() {
        let (inb, out) = (format!("in-{i}"), format!("out-{i}"));
        inbounds.push(
            json!({ "type": "socks", "tag": inb, "listen": "127.0.0.1", "listen_port": port }),
        );
        outbounds.push(outbound(profile, &out)?);
        rules.push(json!({ "inbound": [inb], "outbound": out }));
    }
    outbounds.push(json!({ "type": "direct", "tag": "direct" }));
    Ok(json!({
        "log": { "level": "error" },
        "dns": { "servers": [{ "type": "local", "tag": "local" }] },
        "inbounds": inbounds,
        "outbounds": outbounds,
        "route": { "rules": rules, "final": "direct", "default_domain_resolver": "local" },
    }))
}

/// TUN-only sing-box config that forwards everything to another core's local SOCKS
/// inbound (used to give Xray a TUN mode, like Happ does). Traffic of the backend
/// process itself goes direct to avoid a routing loop.
pub fn tun_front_config(
    opts: &ProxyOptions,
    upstream_host: &str,
    upstream_port: u16,
    bypass_processes: &[String],
) -> anyhow::Result<Value> {
    let tun = opts.tun.as_ref().context("TUN options are required")?;
    let mut rules = vec![
        json!({ "process_name": bypass_processes, "outbound": "direct" }),
        json!({ "action": "sniff" }),
        json!({ "protocol": "dns", "action": "hijack-dns" }),
    ];
    if opts.bypass_lan {
        rules.push(json!({ "ip_is_private": true, "outbound": "direct" }));
    }
    Ok(json!({
        "log": { "level": log_level(&opts.log_level), "timestamp": true },
        "dns": {
            "servers": [
                dns_server(&opts.dns_remote, "remote", Some("proxy"))?,
                { "type": "local", "tag": "local" },
            ],
            "final": "remote",
            "strategy": "prefer_ipv4",
        },
        "inbounds": [{
            "type": "tun",
            "tag": "tun-in",
            "interface_name": tun.interface,
            "address": ["172.19.0.1/30", "fdfe:dcba:9876::1/126"],
            "mtu": tun.mtu,
            "auto_route": true,
            "strict_route": true,
            "stack": tun.stack,
            "iproute2_table_index": TUN_TABLE_INDEX,
            "iproute2_rule_index": TUN_RULE_INDEX,
        }],
        "outbounds": [
            { "type": "socks", "tag": "proxy", "server": upstream_host, "server_port": upstream_port, "version": "5" },
            { "type": "direct", "tag": "direct" },
        ],
        "route": {
            "rules": rules,
            "final": "proxy",
            "auto_detect_interface": true,
            "default_domain_resolver": "local",
        },
    }))
}
