//! Full Xray JSON configs as profiles (what Happ and Streisand get from
//! subscriptions): balancers, observatory and the provider's routing are kept
//! as is, only the local entry points are replaced by ours.
//!
//! A subscription entry like "LTE Авто - Нидерланды" is a balancer over several
//! servers. Exported as a share link it degrades to a single server (the
//! balancer's fallback), which may well be dead — so these configs are run whole.

use anyhow::{Context, bail};
use serde_json::{Value, json};

use crate::model::{Profile, Protocol};

/// Tag of our local inbound; also used by the TUN front.
pub const INBOUND_TAG: &str = "mixed-in";
/// Tag given to Xray's built-in DNS client so it can be routed.
const DNS_TAG: &str = "rustbox-dns";
const DIRECT_TAG: &str = "rustbox-direct";

/// Local proxy entry points of the original config: replaced by ours.
const LOCAL_INBOUNDS: &[&str] = &["socks", "http", "mixed", "tun"];
/// Outbounds that are not servers.
const SERVICE_OUTBOUNDS: &[&str] = &["freedom", "blackhole", "dns", "loopback"];

/// Parses a subscription body that is a JSON array of configs (or one config).
/// Returns `None` when the body is not an Xray JSON config at all.
pub fn parse_configs(body: &str) -> Option<(Vec<Profile>, Vec<String>)> {
    let value: Value = serde_json::from_str(body).ok()?;
    let items = match value {
        Value::Array(items) => items,
        obj @ Value::Object(_) => vec![obj],
        _ => return None,
    };
    if !items.iter().any(|c| c.get("outbounds").is_some()) {
        return None;
    }
    let mut profiles = Vec::new();
    let mut errors = Vec::new();
    for (i, config) in items.into_iter().enumerate() {
        let name = remarks(&config).unwrap_or_else(|| format!("#{}", i + 1));
        match profile_from_config(config) {
            Ok(p) => profiles.push(p),
            Err(e) => errors.push(format!("{name}: {e:#}")),
        }
    }
    Some((profiles, errors))
}

fn remarks(config: &Value) -> Option<String> {
    config
        .get("remarks")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

pub fn profile_from_config(config: Value) -> anyhow::Result<Profile> {
    let outbounds = config
        .get("outbounds")
        .and_then(Value::as_array)
        .context("config has no outbounds")?;
    let primary = primary_outbound(&config).context("config has no server outbound")?;
    let (server, port) = outbound_address(primary).with_context(|| {
        format!(
            "cannot find the server address of outbound '{}'",
            primary.get("tag").and_then(Value::as_str).unwrap_or("?")
        )
    })?;
    anyhow::ensure!(
        outbounds.iter().any(is_server),
        "config has no server outbound"
    );
    let mut profile = Profile::new(
        server,
        port,
        Protocol::Xray {
            config: Value::Null,
        },
    );
    profile.name = remarks(&config).unwrap_or_default();
    profile.protocol = Protocol::Xray { config };
    Ok(profile)
}

fn is_server(outbound: &Value) -> bool {
    outbound
        .get("protocol")
        .and_then(Value::as_str)
        .is_some_and(|p| !SERVICE_OUTBOUNDS.contains(&p))
}

fn outbounds(config: &Value) -> impl Iterator<Item = &Value> {
    config
        .get("outbounds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// Server outbounds of a config.
pub fn servers(config: &Value) -> Vec<&Value> {
    outbounds(config).filter(|o| is_server(o)).collect()
}

/// The outbound that describes the profile: the first candidate of the first
/// balancer (Xray selectors are tag prefixes), otherwise the first server.
pub fn primary_outbound(config: &Value) -> Option<&Value> {
    let first_selector = config
        .pointer("/routing/balancers/0/selector/0")
        .and_then(Value::as_str);
    if let Some(prefix) = first_selector
        && let Some(o) = servers(config).into_iter().find(|o| {
            o.get("tag")
                .and_then(Value::as_str)
                .is_some_and(|t| t.starts_with(prefix))
        })
    {
        return Some(o);
    }
    servers(config).into_iter().next()
}

fn outbound_address(o: &Value) -> Option<(String, u16)> {
    let s = o.get("settings")?;
    let target = s
        .pointer("/vnext/0")
        .or_else(|| s.pointer("/servers/0"))
        .unwrap_or(s);
    let server = target.get("address")?.as_str()?.to_string();
    let port = target
        .get("port")?
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())?;
    Some((server, port))
}

/// Short description for the table: "Авто · 7 серверов" for balancers,
/// "VLESS+Reality" / "VLESS+TLS/grpc" for a single server.
pub fn describe(config: &Value) -> String {
    let n = servers(config).len();
    let has_balancer = config.pointer("/routing/balancers/0").is_some();
    if has_balancer || n > 1 {
        return format!("Авто · {n} {}", servers_word(n));
    }
    primary_outbound(config)
        .map(describe_outbound)
        .unwrap_or_else(|| "Xray".into())
}

fn servers_word(n: usize) -> &'static str {
    match (n % 10, n % 100) {
        (1, r) if r != 11 => "сервер",
        (2..=4, r) if !(12..=14).contains(&r) => "сервера",
        _ => "серверов",
    }
}

fn describe_outbound(o: &Value) -> String {
    let proto = match o.get("protocol").and_then(Value::as_str).unwrap_or("?") {
        "vless" => "VLESS",
        "vmess" => "VMess",
        "trojan" => "Trojan",
        "shadowsocks" => "Shadowsocks",
        "hysteria" => "Hysteria2",
        "wireguard" => "WireGuard",
        other => other,
    }
    .to_string();
    let stream = o.get("streamSettings");
    let get = |k: &str| stream.and_then(|s| s.get(k)).and_then(Value::as_str);
    let mut s = proto;
    match get("security") {
        Some("reality") => s.push_str("+Reality"),
        Some("tls") if !s.starts_with("Hysteria") => s.push_str("+TLS"),
        _ => {}
    }
    if let Some(net) = get("network").filter(|n| !matches!(*n, "tcp" | "raw" | "hysteria")) {
        s.push('/');
        s.push_str(net);
    }
    s
}

/// Where local traffic goes in the original config: the target of the rule
/// that catches everything from the local inbounds (balancer or outbound).
fn main_target(rules: &[Value], inbound_tag: &str) -> Option<(&'static str, Value)> {
    const CATCH_ALL_KEYS: &[&str] = &[
        "type",
        "inboundTag",
        "network",
        "balancerTag",
        "outboundTag",
        "ruleTag",
    ];
    rules.iter().find_map(|r| {
        let obj = r.as_object()?;
        let from_local = obj
            .get("inboundTag")?
            .as_array()?
            .iter()
            .any(|t| t.as_str() == Some(inbound_tag));
        if !from_local || !obj.keys().all(|k| CATCH_ALL_KEYS.contains(&k.as_str())) {
            return None;
        }
        obj.get("balancerTag")
            .map(|t| ("balancerTag", t.clone()))
            .or_else(|| obj.get("outboundTag").map(|t| ("outboundTag", t.clone())))
    })
}

pub struct Session<'a> {
    /// Our local entry point.
    pub inbound: Value,
    pub log_level: &'a str,
    pub bypass_lan: bool,
}

/// Mixed HTTP+SOCKS inbound for a session.
pub fn mixed_inbound(listen: &str, port: u16) -> Value {
    json!({
        "tag": INBOUND_TAG,
        "listen": listen,
        "port": port,
        "protocol": "mixed",
        "settings": { "udp": true },
        "sniffing": { "enabled": true, "destOverride": ["http", "tls", "quic"], "routeOnly": true },
    })
}

/// SOCKS inbound for URL tests.
pub fn test_inbound(port: u16) -> Value {
    json!({
        "tag": INBOUND_TAG, "listen": "127.0.0.1", "port": port,
        "protocol": "socks", "settings": { "udp": false },
        "sniffing": { "enabled": true, "destOverride": ["http", "tls"], "routeOnly": true },
    })
}

/// The provider's config, ready to run as our session:
/// * local inbounds (socks/http/…) are replaced by `session.inbound`, and rules
///   that referenced them now reference ours — otherwise traffic would miss the
///   provider's catch-all rule and go to the first outbound;
/// * Xray's own DNS client is routed like the main traffic. Without a rule it
///   uses the first outbound, which in balancer configs is the fallback server
///   and often dead: every direct-routed site then waits for DNS timeouts;
/// * optional LAN bypass; our log level; bookkeeping keys removed.
pub fn session_config(config: &Value, session: &Session<'_>) -> anyhow::Result<Value> {
    let mut c = config.clone();
    let obj = c.as_object_mut().context("config is not a JSON object")?;
    obj.remove("remarks");
    obj.remove("meta");
    obj.insert(
        "log".into(),
        json!({ "loglevel": xray_log_level(session.log_level), "access": "none" }),
    );

    // Inbounds.
    let mut replaced = Vec::new();
    let mut inbounds = Vec::new();
    for inb in obj
        .get("inbounds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let local = inb
            .get("protocol")
            .and_then(Value::as_str)
            .is_some_and(|p| LOCAL_INBOUNDS.contains(&p));
        if local {
            if let Some(tag) = inb.get("tag").and_then(Value::as_str) {
                replaced.push(tag.to_string());
            }
        } else {
            inbounds.push(inb);
        }
    }
    inbounds.insert(0, session.inbound.clone());
    obj.insert("inbounds".into(), Value::Array(inbounds));

    // Rules that pointed at the replaced inbounds now point at ours.
    let routing = obj
        .entry("routing")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("routing is not an object")?;
    let rules = routing
        .entry("rules")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("routing.rules is not an array")?;
    for rule in rules.iter_mut() {
        let Some(tags) = rule.get_mut("inboundTag").and_then(Value::as_array_mut) else {
            continue;
        };
        if tags
            .iter()
            .any(|t| t.as_str().is_some_and(|t| replaced.iter().any(|r| r == t)))
        {
            tags.retain(|t| !t.as_str().is_some_and(|t| replaced.iter().any(|r| r == t)));
            tags.push(json!(INBOUND_TAG));
        }
    }

    let mut prepend = Vec::new();
    if session.bypass_lan {
        prepend.push(json!({ "type": "field", "ip": crate::config::PRIVATE_CIDRS, "outboundTag": "__direct__" }));
    }
    let target = c
        .pointer("/routing/rules")
        .and_then(Value::as_array)
        .and_then(|rules| main_target(rules, INBOUND_TAG));
    if let Some((key, target)) = target
        && let Some(dns) = obj_dns_tag(&mut c)
    {
        prepend.push(json!({ "type": "field", "inboundTag": [dns], key: target }));
    }
    if !prepend.is_empty() {
        let direct = if prepend.iter().any(|r| r["outboundTag"] == "__direct__") {
            Some(ensure_direct(&mut c))
        } else {
            None
        };
        let rules = c
            .pointer_mut("/routing/rules")
            .and_then(Value::as_array_mut)
            .expect("created above");
        for mut rule in prepend.into_iter().rev() {
            if rule["outboundTag"] == "__direct__" {
                rule["outboundTag"] = json!(direct.clone().unwrap_or_default());
            }
            rules.insert(0, rule);
        }
    }
    Ok(c)
}

/// Tags the config's DNS section (keeping an existing tag); `None` without one.
fn obj_dns_tag(c: &mut Value) -> Option<String> {
    let dns = c.get_mut("dns")?.as_object_mut()?;
    let tag = dns
        .get("tag")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| DNS_TAG.to_string());
    dns.insert("tag".into(), json!(tag));
    Some(tag)
}

/// Tag of a freedom outbound, adding one if the config has none.
fn ensure_direct(c: &mut Value) -> String {
    let existing = outbounds(c).find_map(|o| {
        (o.get("protocol").and_then(Value::as_str) == Some("freedom"))
            .then(|| o.get("tag").and_then(Value::as_str).map(String::from))
            .flatten()
    });
    if let Some(tag) = existing {
        return tag;
    }
    if let Some(list) = c.get_mut("outbounds").and_then(Value::as_array_mut) {
        list.push(json!({ "tag": DIRECT_TAG, "protocol": "freedom" }));
    }
    DIRECT_TAG.to_string()
}

fn xray_log_level(level: &str) -> &str {
    match level {
        "debug" | "info" | "error" => level,
        _ => "warning",
    }
}

/// Validates that a config can be turned into a session.
pub fn check(config: &Value) -> anyhow::Result<()> {
    if servers(config).is_empty() {
        bail!("config has no server outbound");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape of a provider's balancer config (Happ format), with fake data.
    fn balancer_config() -> Value {
        json!({
            "remarks": "🇳🇱 Авто - Нидерланды",
            "dns": { "servers": ["8.8.8.8", "1.1.1.1"], "queryStrategy": "UseIP" },
            "inbounds": [
                { "tag": "socks", "listen": "127.0.0.1", "port": 10808, "protocol": "socks", "settings": { "udp": true } },
                { "tag": "http", "listen": "127.0.0.1", "port": 10809, "protocol": "http" }
            ],
            "observatory": { "subjectSelector": ["cand-02", "cand-03"], "probeUrl": "https://www.google.com/generate_204", "probeInterval": "2m" },
            "outbounds": [
                { "tag": "cand-01", "protocol": "vless",
                  "settings": { "vnext": [{ "address": "10.0.0.1", "port": 443, "users": [{ "id": "00000000-0000-0000-0000-000000000001", "encryption": "none" }] }] },
                  "streamSettings": { "network": "grpc", "security": "tls", "grpcSettings": { "serviceName": "svc" }, "tlsSettings": { "serverName": "a.example" } } },
                { "tag": "cand-02", "protocol": "vless",
                  "settings": { "vnext": [{ "address": "10.0.0.2", "port": 8443, "users": [{ "id": "00000000-0000-0000-0000-000000000001", "encryption": "none", "flow": "xtls-rprx-vision" }] }] },
                  "streamSettings": { "network": "tcp", "security": "reality", "realitySettings": { "serverName": "b.example", "publicKey": "Z84J2IelR9ch3k8VtlVhhs5ycBUlXA7wHBWcBrjqnAw", "shortId": "abcd", "fingerprint": "chrome" } } },
                { "tag": "cand-03", "protocol": "hysteria", "settings": { "address": "10.0.0.3", "port": 30443, "version": 2 } },
                { "tag": "direct", "protocol": "freedom" },
                { "tag": "block", "protocol": "blackhole" }
            ],
            "routing": {
                "domainStrategy": "IPIfNonMatch",
                "balancers": [{ "tag": "bal", "selector": ["cand-02", "cand-03"], "fallbackTag": "cand-01", "strategy": { "type": "leastLoad" } }],
                "rules": [
                    { "type": "field", "protocol": ["quic"], "outboundTag": "block" },
                    { "type": "field", "domain": ["domain:ya.ru"], "outboundTag": "direct" },
                    { "type": "field", "inboundTag": ["socks", "http"], "network": "tcp,udp", "balancerTag": "bal" }
                ]
            }
        })
    }

    fn single_config() -> Value {
        json!({
            "remarks": "Быстрый",
            "inbounds": [{ "tag": "socks", "port": 10808, "protocol": "socks" }],
            "outbounds": [
                { "tag": "proxy", "protocol": "vless",
                  "settings": { "vnext": [{ "address": "example.com", "port": 443, "users": [{ "id": "00000000-0000-0000-0000-000000000001", "encryption": "none" }] }] },
                  "streamSettings": { "network": "raw", "security": "reality", "realitySettings": { "serverName": "b.example", "publicKey": "Z84J2IelR9ch3k8VtlVhhs5ycBUlXA7wHBWcBrjqnAw", "shortId": "abcd" } } },
                { "tag": "direct", "protocol": "freedom" }
            ],
            "routing": { "rules": [] }
        })
    }

    fn session(bypass_lan: bool) -> Session<'static> {
        Session {
            inbound: mixed_inbound("127.0.0.1", 20808),
            log_level: "warn",
            bypass_lan,
        }
    }

    #[test]
    fn parses_subscription_array() {
        let body = json!([balancer_config(), single_config(), { "outbounds": [] }]).to_string();
        let (profiles, errors) = parse_configs(&body).unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(errors.len(), 1);

        // Balancer: named by remarks, address of the first selector candidate,
        // not of the fallback that a share link would contain.
        let p = &profiles[0];
        assert_eq!(p.name, "🇳🇱 Авто - Нидерланды");
        assert_eq!((p.server.as_str(), p.port), ("10.0.0.2", 8443));
        assert_eq!(p.type_label(), "Авто · 3 сервера");

        assert_eq!(profiles[1].type_label(), "VLESS+Reality");
        assert_eq!(profiles[1].address(), "example.com:443");
    }

    #[test]
    fn not_json_or_not_xray_is_none() {
        assert!(parse_configs("vless://x@y:1#z").is_none());
        assert!(parse_configs(r#"{"proxies": []}"#).is_none());
    }

    #[test]
    fn session_replaces_local_inbounds_and_rewrites_rules() {
        let c = session_config(&balancer_config(), &session(false)).unwrap();
        let inbounds = c["inbounds"].as_array().unwrap();
        assert_eq!(inbounds.len(), 1);
        assert_eq!(inbounds[0]["tag"], INBOUND_TAG);
        assert!(c.get("remarks").is_none());

        let rules = c["routing"]["rules"].as_array().unwrap();
        let catch_all = rules.iter().find(|r| r.get("network").is_some()).unwrap();
        assert_eq!(catch_all["inboundTag"], json!([INBOUND_TAG]));
        assert_eq!(catch_all["balancerTag"], "bal");
    }

    #[test]
    fn builtin_dns_follows_main_traffic() {
        let c = session_config(&balancer_config(), &session(false)).unwrap();
        assert_eq!(c["dns"]["tag"], DNS_TAG);
        let first = &c["routing"]["rules"][0];
        assert_eq!(first["inboundTag"], json!([DNS_TAG]));
        assert_eq!(
            first["balancerTag"], "bal",
            "DNS must not go to the dead fallback"
        );
    }

    #[test]
    fn bypass_lan_uses_existing_freedom() {
        let c = session_config(&balancer_config(), &session(true)).unwrap();
        let first = &c["routing"]["rules"][0];
        assert_eq!(first["outboundTag"], "direct");
        assert!(
            first["ip"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "192.168.0.0/16")
        );

        let mut no_freedom = single_config();
        no_freedom["outbounds"].as_array_mut().unwrap().pop();
        let c = session_config(&no_freedom, &session(true)).unwrap();
        assert_eq!(c["routing"]["rules"][0]["outboundTag"], DIRECT_TAG);
        assert!(
            c["outbounds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["tag"] == DIRECT_TAG)
        );
    }

    #[test]
    fn session_configs_are_accepted_by_xray() {
        let platform = rustbox_platform::current();
        let Some(exe) = platform.find_executable("xray") else {
            eprintln!("xray not installed, skipping validation");
            return;
        };
        let dir = std::env::temp_dir().join(format!("rustbox-xrayjson-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, config) in [("balancer", balancer_config()), ("single", single_config())] {
            let c = session_config(&config, &session(true)).unwrap();
            let path = dir.join(format!("{name}.json"));
            std::fs::write(&path, serde_json::to_vec_pretty(&c).unwrap()).unwrap();
            let r = crate::process::check_config(
                crate::CoreKind::Xray,
                &exe,
                &path,
                &platform.core_environment(false),
            );
            if let Err(e) = r {
                let _ = std::fs::remove_dir_all(&dir);
                panic!(
                    "xray rejected {name}: {e:#}\n{}",
                    serde_json::to_string_pretty(&c).unwrap()
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn configs_without_catch_all_are_left_alone() {
        // No inbound rule: traffic goes to the first outbound ("proxy"), as in Happ.
        let c = session_config(&single_config(), &session(false)).unwrap();
        assert_eq!(c["routing"]["rules"], json!([]));
    }
}
