//! Xray-core config generator.

use anyhow::bail;
use serde_json::{Value, json};

use super::dns::DnsServer;
use super::{PRIVATE_CIDRS, ProxyOptions};
use crate::model::{Profile, Protocol, Transport};

fn log_level(level: &str) -> &str {
    match level {
        "debug" | "info" | "error" => level,
        "warn" | "warning" => "warning",
        _ => "warning",
    }
}

fn stream_settings(p: &Profile) -> anyhow::Result<Value> {
    let mut s = json!({ "network": "raw" });
    match &p.transport {
        Transport::Tcp => {}
        Transport::Ws { path, host } => {
            s["network"] = json!("ws");
            s["wsSettings"] = json!({ "path": path, "host": host.clone().unwrap_or_default() });
        }
        Transport::Grpc { service_name } => {
            s["network"] = json!("grpc");
            s["grpcSettings"] = json!({ "serviceName": service_name });
        }
        Transport::HttpUpgrade { path, host } => {
            s["network"] = json!("httpupgrade");
            s["httpupgradeSettings"] =
                json!({ "path": path, "host": host.clone().unwrap_or_default() });
        }
        Transport::Xhttp { path, host, mode } => {
            s["network"] = json!("xhttp");
            s["xhttpSettings"] = json!({
                "path": path,
                "host": host.clone().unwrap_or_default(),
                "mode": mode.clone().unwrap_or_else(|| "auto".into()),
            });
        }
        Transport::Http { .. } => bail!("HTTP/2 transport was removed from Xray, use sing-box"),
    }
    match &p.tls {
        None => s["security"] = json!("none"),
        Some(tls) => {
            let sni = tls.sni.clone().unwrap_or_else(|| p.server.clone());
            if let Some(r) = &tls.reality {
                s["security"] = json!("reality");
                s["realitySettings"] = json!({
                    "serverName": sni,
                    "fingerprint": tls.fingerprint.clone().unwrap_or_else(|| "chrome".into()),
                    "publicKey": r.public_key,
                    "shortId": r.short_id,
                    "spiderX": r.spider_x.clone().unwrap_or_default(),
                });
            } else {
                s["security"] = json!("tls");
                let mut t = json!({ "serverName": sni });
                if tls.insecure {
                    t["allowInsecure"] = json!(true);
                }
                if !tls.alpn.is_empty() {
                    t["alpn"] = json!(tls.alpn);
                }
                if let Some(fp) = &tls.fingerprint {
                    t["fingerprint"] = json!(fp);
                }
                if !tls.pinned_cert_sha256.is_empty() {
                    t["pinnedPeerCertSha256"] = json!(tls.pinned_cert_sha256.join(","));
                }
                s["tlsSettings"] = t;
            }
        }
    }
    Ok(s)
}

pub fn outbound(p: &Profile, tag: &str) -> anyhow::Result<Value> {
    let (protocol, settings) = match &p.protocol {
        Protocol::Vmess {
            uuid,
            alter_id,
            security,
        } => (
            "vmess",
            json!({ "vnext": [{ "address": p.server, "port": p.port,
                "users": [{ "id": uuid, "alterId": alter_id, "security": security }] }] }),
        ),
        Protocol::Vless { uuid, flow } => (
            "vless",
            json!({ "vnext": [{ "address": p.server, "port": p.port,
                "users": [{ "id": uuid, "encryption": "none", "flow": flow.clone().unwrap_or_default() }] }] }),
        ),
        Protocol::Trojan { password } => (
            "trojan",
            json!({ "servers": [{ "address": p.server, "port": p.port, "password": password }] }),
        ),
        Protocol::Shadowsocks {
            method,
            password,
            plugin,
            ..
        } => {
            if plugin.is_some() {
                bail!("shadowsocks plugins are not supported by Xray, use sing-box");
            }
            (
                "shadowsocks",
                json!({ "servers": [{ "address": p.server, "port": p.port, "method": method, "password": password }] }),
            )
        }
        Protocol::Socks { username, password } | Protocol::Http { username, password } => {
            let mut server = json!({ "address": p.server, "port": p.port });
            if let Some(u) = username {
                server["users"] =
                    json!([{ "user": u, "pass": password.clone().unwrap_or_default() }]);
            }
            let proto = if matches!(p.protocol, Protocol::Socks { .. }) {
                "socks"
            } else {
                "http"
            };
            (proto, json!({ "servers": [server] }))
        }
        Protocol::Hysteria2 { .. } | Protocol::Tuic { .. } => {
            bail!(
                "{} is not supported by Xray, use sing-box",
                p.protocol.name()
            )
        }
        Protocol::Xray { .. } => bail!("a full Xray config cannot be used as a single outbound"),
    };
    Ok(json!({
        "tag": tag,
        "protocol": protocol,
        "settings": settings,
        "streamSettings": stream_settings(p)?,
    }))
}

pub fn full_config(p: &Profile, opts: &ProxyOptions) -> anyhow::Result<Value> {
    let mut rules = Vec::new();
    if opts.bypass_lan {
        rules.push(json!({ "ip": PRIVATE_CIDRS, "outboundTag": "direct" }));
    }
    rules.push(json!({ "inboundTag": ["mixed-in"], "outboundTag": "proxy" }));

    Ok(json!({
        // Access log would print every connection; errors stay visible.
        "log": { "loglevel": log_level(&opts.log_level), "access": "none" },
        "dns": { "servers": [DnsServer::parse(&opts.dns_remote)?.to_url()] },
        "inbounds": [{
            "tag": "mixed-in",
            "listen": opts.listen,
            "port": opts.port,
            "protocol": "mixed",
            "settings": { "udp": true },
            "sniffing": { "enabled": true, "destOverride": ["http", "tls", "quic"], "routeOnly": true },
        }],
        "outbounds": [
            outbound(p, "proxy")?,
            { "tag": "direct", "protocol": "freedom" },
        ],
        "routing": { "domainStrategy": "AsIs", "rules": rules },
    }))
}

pub fn test_config(entries: &[(u16, &Profile)]) -> anyhow::Result<Value> {
    let mut inbounds = Vec::new();
    let mut outbounds = Vec::new();
    let mut rules = Vec::new();
    for (i, (port, profile)) in entries.iter().enumerate() {
        let (inb, out) = (format!("in-{i}"), format!("out-{i}"));
        inbounds.push(
            json!({ "tag": inb, "listen": "127.0.0.1", "port": port, "protocol": "socks",
            "settings": { "udp": false } }),
        );
        outbounds.push(outbound(profile, &out)?);
        rules.push(json!({ "inboundTag": [inb], "outboundTag": out }));
    }
    Ok(json!({
        "log": { "loglevel": "error" },
        "inbounds": inbounds,
        "outbounds": outbounds,
        "routing": { "rules": rules },
    }))
}
