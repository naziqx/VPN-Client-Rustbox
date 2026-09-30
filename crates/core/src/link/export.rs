use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::json;

use crate::model::{Profile, Protocol, Transport};

/// Characters left as-is when percent-encoding link components.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn enc(s: &str) -> String {
    utf8_percent_encode(s, COMPONENT).to_string()
}

fn host(server: &str) -> String {
    if server.contains(':') {
        format!("[{server}]")
    } else {
        server.to_string()
    }
}

/// Serializes a profile back to a share link understood by NekoBox / v2rayN.
pub fn to_link(p: &Profile) -> String {
    match &p.protocol {
        // No share-link form: the config itself is what Happ/v2rayN import.
        Protocol::Xray { config } => serde_json::to_string(config).unwrap_or_default(),
        Protocol::Vmess {
            uuid,
            alter_id,
            security,
        } => vmess_link(p, uuid, *alter_id, security),
        Protocol::Vless { uuid, flow } => {
            let mut q = common_query(p);
            q.insert(0, ("encryption".into(), "none".into()));
            if let Some(flow) = flow {
                q.push(("flow".into(), flow.clone()));
            }
            build("vless", Some(uuid), p, &q)
        }
        Protocol::Trojan { password } => build("trojan", Some(password), p, &common_query(p)),
        Protocol::Shadowsocks {
            method,
            password,
            plugin,
            plugin_opts,
        } => {
            let userinfo = URL_SAFE_NO_PAD.encode(format!("{method}:{password}"));
            let mut link = format!("ss://{userinfo}@{}:{}", host(&p.server), p.port);
            if let Some(plugin) = plugin {
                let full = match plugin_opts {
                    Some(opts) => format!("{plugin};{opts}"),
                    None => plugin.clone(),
                };
                link.push_str(&format!("/?plugin={}", enc(&full)));
            }
            link.push('#');
            link.push_str(&enc(&p.name));
            link
        }
        Protocol::Hysteria2 {
            password,
            obfs_password,
            ..
        } => {
            let mut q = tls_query(p);
            if let Some(obfs) = obfs_password {
                q.push(("obfs".into(), "salamander".into()));
                q.push(("obfs-password".into(), obfs.clone()));
            }
            build("hysteria2", Some(password), p, &q)
        }
        Protocol::Tuic {
            uuid,
            password,
            congestion_control,
            udp_relay_mode,
        } => {
            let mut q = tls_query(p);
            q.push(("congestion_control".into(), congestion_control.clone()));
            q.push(("udp_relay_mode".into(), udp_relay_mode.clone()));
            let userinfo = format!("{}:{}", enc(uuid), enc(password));
            build_raw("tuic", Some(userinfo), p, &q)
        }
        Protocol::Socks { username, password } | Protocol::Http { username, password } => {
            let scheme = if matches!(p.protocol, Protocol::Socks { .. }) {
                "socks"
            } else {
                "http"
            };
            let userinfo = username.as_ref().map(|u| {
                STANDARD.encode(format!("{u}:{}", password.as_deref().unwrap_or_default()))
            });
            build_raw(scheme, userinfo, p, &[])
        }
    }
}

fn build(scheme: &str, userinfo: Option<&String>, p: &Profile, q: &[(String, String)]) -> String {
    build_raw(scheme, userinfo.map(|u| enc(u)), p, q)
}

fn build_raw(
    scheme: &str,
    userinfo: Option<String>,
    p: &Profile,
    q: &[(String, String)],
) -> String {
    let mut link = format!("{scheme}://");
    if let Some(u) = userinfo {
        link.push_str(&u);
        link.push('@');
    }
    link.push_str(&format!("{}:{}", host(&p.server), p.port));
    if !q.is_empty() {
        let query: Vec<String> = q.iter().map(|(k, v)| format!("{k}={}", enc(v))).collect();
        link.push('?');
        link.push_str(&query.join("&"));
    }
    link.push('#');
    link.push_str(&enc(&p.name));
    link
}

fn tls_query(p: &Profile) -> Vec<(String, String)> {
    let mut q = Vec::new();
    let Some(tls) = &p.tls else {
        return q;
    };
    if let Some(sni) = &tls.sni {
        q.push(("sni".into(), sni.clone()));
    }
    if !tls.alpn.is_empty() {
        q.push(("alpn".into(), tls.alpn.join(",")));
    }
    if let Some(fp) = &tls.fingerprint {
        q.push(("fp".into(), fp.clone()));
    }
    if tls.insecure {
        q.push(("insecure".into(), "1".into()));
    }
    if !tls.pinned_cert_sha256.is_empty() {
        q.push(("pcs".into(), tls.pinned_cert_sha256.join(",")));
    }
    if let Some(r) = &tls.reality {
        q.push(("pbk".into(), r.public_key.clone()));
        q.push(("sid".into(), r.short_id.clone()));
        if let Some(spx) = &r.spider_x {
            q.push(("spx".into(), spx.clone()));
        }
    }
    q
}

fn common_query(p: &Profile) -> Vec<(String, String)> {
    let security = match &p.tls {
        Some(t) if t.reality.is_some() => "reality",
        Some(_) => "tls",
        None => "none",
    };
    let mut q = vec![("security".to_string(), security.to_string())];
    q.extend(tls_query(p));
    q.push(("type".into(), p.transport.name().into()));
    match &p.transport {
        Transport::Tcp => {}
        Transport::Grpc { service_name } => q.push(("serviceName".into(), service_name.clone())),
        Transport::Ws { path, host }
        | Transport::Http { path, host }
        | Transport::HttpUpgrade { path, host }
        | Transport::Xhttp { path, host, .. } => {
            q.push(("path".into(), path.clone()));
            if let Some(h) = host {
                q.push(("host".into(), h.clone()));
            }
            if let Transport::Xhttp {
                mode: Some(mode), ..
            } = &p.transport
            {
                q.push(("mode".into(), mode.clone()));
            }
        }
    }
    q
}

fn vmess_link(p: &Profile, uuid: &str, alter_id: u16, security: &str) -> String {
    let (net, path, host) = match &p.transport {
        Transport::Tcp => ("tcp", String::new(), None),
        Transport::Ws { path, host } => ("ws", path.clone(), host.clone()),
        Transport::Grpc { service_name } => ("grpc", service_name.clone(), None),
        Transport::Http { path, host } => ("h2", path.clone(), host.clone()),
        Transport::HttpUpgrade { path, host } => ("httpupgrade", path.clone(), host.clone()),
        Transport::Xhttp { path, host, .. } => ("xhttp", path.clone(), host.clone()),
    };
    let tls = p.tls.as_ref();
    let v = json!({
        "v": "2",
        "ps": p.name,
        "add": p.server,
        "port": p.port.to_string(),
        "id": uuid,
        "aid": alter_id.to_string(),
        "scy": security,
        "net": net,
        "type": "none",
        "host": host.unwrap_or_default(),
        "path": path,
        "tls": if tls.is_some() { "tls" } else { "" },
        "sni": tls.and_then(|t| t.sni.clone()).unwrap_or_default(),
        "alpn": tls.map(|t| t.alpn.join(",")).unwrap_or_default(),
        "fp": tls.and_then(|t| t.fingerprint.clone()).unwrap_or_default(),
    });
    format!("vmess://{}", STANDARD.encode(v.to_string()))
}
