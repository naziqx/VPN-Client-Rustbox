//! Generated configs are validated by the real cores when they are installed.

use std::path::PathBuf;

use super::*;
use crate::link::parse_link;
use crate::process::check_config;

const UUID: &str = "b831381d-6324-4d53-ad4f-8cda48b30811";

fn samples() -> Vec<Profile> {
    [
        format!("vless://{UUID}@example.com:443?security=reality&sni=www.microsoft.com&fp=chrome&pbk=Z84J2IelR9ch3k8VtlVhhs5ycBUlXA7wHBWcBrjqnAw&sid=6ba85179e30d4fc2&type=tcp&flow=xtls-rprx-vision#reality"),
        format!("vless://{UUID}@example.com:443?security=tls&sni=a.com&type=ws&path=%2Fws%3Fed%3D2048&host=a.com#ws"),
        format!("vless://{UUID}@example.com:443?security=tls&type=grpc&serviceName=g#grpc"),
        format!("vless://{UUID}@example.com:443?security=tls&type=httpupgrade&path=%2Fu&host=a.com#hu"),
        "trojan://pw@example.com:443?sni=a.com#trojan".to_string(),
        "ss://YWVzLTI1Ni1nY206cGFzcw@1.2.3.4:8388#ss".to_string(),
        "socks://dXNlcjpwYXNz@1.2.3.4:1080#socks".to_string(),
    ]
    .iter()
    .map(|l| parse_link(l).unwrap())
    .collect()
}

fn singbox_only() -> Vec<Profile> {
    [
        "hysteria2://pw@example.com:443?sni=a.com&obfs=salamander&obfs-password=o#hy2".to_string(),
        format!("tuic://{UUID}:pw@example.com:443?sni=a.com&alpn=h3#tuic"),
        "ss://YWVzLTI1Ni1nY206cGFzcw@1.2.3.4:8388/?plugin=obfs-local%3Bobfs%3Dhttp#ssobfs"
            .to_string(),
    ]
    .iter()
    .map(|l| parse_link(l).unwrap())
    .collect()
}

fn xray_only() -> Vec<Profile> {
    vec![
        parse_link(&format!(
            "vless://{UUID}@example.com:443?security=tls&type=xhttp&path=%2Fx&mode=auto#xhttp"
        ))
        .unwrap(),
    ]
}

fn pinned() -> Profile {
    let pcs = "ab".repeat(32);
    parse_link(&format!(
        "vless://{UUID}@1.2.3.4:443?type=grpc&serviceName=svc&security=tls&sni=lk.x5.ru&fp=edge&pcs={pcs}#pinned"
    ))
    .unwrap()
}

#[test]
fn pinned_configs_are_valid() {
    let p = pinned();
    validate(
        CoreKind::Xray,
        "pinned",
        &CoreKind::Xray.build_config(&p, &opts()).unwrap(),
    );
    // sing-box needs the public-key pin resolved first.
    assert!(CoreKind::SingBox.build_config(&p, &opts()).is_err());
    assert!(CoreKind::SingBox.check_profile(&p).is_ok());
    let mut resolved = p.clone();
    resolved.tls.as_mut().unwrap().pinned_pubkey_sha256 =
        vec!["47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=".into()];
    validate(
        CoreKind::SingBox,
        "pinned",
        &CoreKind::SingBox.build_config(&resolved, &opts()).unwrap(),
    );
}

fn opts() -> ProxyOptions {
    ProxyOptions {
        listen: "127.0.0.1".into(),
        port: 20808,
        tun: None,
        bypass_lan: true,
        dns_remote: "https://1.1.1.1/dns-query".into(),
        dns_direct: "local".into(),
        log_level: "warn".into(),
    }
}

fn core_exe(kind: CoreKind) -> Option<PathBuf> {
    rustbox_platform::current().find_executable(kind.executable_name())
}

fn validate(kind: CoreKind, name: &str, config: &serde_json::Value) {
    let Some(exe) = core_exe(kind) else {
        eprintln!("{kind} not installed, skipping validation");
        return;
    };
    let dir =
        std::env::temp_dir().join(format!("rustbox-test-{}-{kind}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{kind}-{name}.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(config).unwrap()).unwrap();
    let env = rustbox_platform::current().core_environment(false);
    let result = check_config(kind, &exe, &path, &env);
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        panic!(
            "{kind} rejected config for {name}: {e:#}\n{}",
            serde_json::to_string_pretty(config).unwrap()
        );
    }
}

#[test]
fn singbox_configs_are_valid() {
    let mut tun_opts = opts();
    tun_opts.tun = Some(TunOptions {
        interface: "rb-test".into(),
        stack: "mixed".into(),
        mtu: 9000,
    });
    tun_opts.dns_remote = "tls://dns.google".into();
    tun_opts.dns_direct = "77.88.8.8".into();
    for p in samples().iter().chain(singbox_only().iter()) {
        validate(
            CoreKind::SingBox,
            &p.name,
            &CoreKind::SingBox.build_config(p, &opts()).unwrap(),
        );
        validate(
            CoreKind::SingBox,
            &format!("{}-tun", p.name),
            &CoreKind::SingBox.build_config(p, &tun_opts).unwrap(),
        );
    }
    for p in xray_only() {
        assert!(CoreKind::SingBox.check_profile(&p).is_err());
    }
}

#[test]
fn xray_configs_are_valid() {
    for p in samples().iter().chain(xray_only().iter()) {
        validate(
            CoreKind::Xray,
            &p.name,
            &CoreKind::Xray.build_config(p, &opts()).unwrap(),
        );
    }
    for p in singbox_only() {
        assert!(
            CoreKind::Xray.check_profile(&p).is_err(),
            "{} should be rejected",
            p.name
        );
    }
    let mut tun = opts();
    tun.tun = Some(TunOptions {
        interface: "x".into(),
        stack: "mixed".into(),
        mtu: 1500,
    });
    assert!(CoreKind::Xray.build_config(&samples()[0], &tun).is_err());
}

#[test]
fn test_configs_are_valid() {
    for kind in CoreKind::ALL {
        let profiles: Vec<Profile> = samples();
        let entries: Vec<(u16, &Profile)> = profiles
            .iter()
            .enumerate()
            .map(|(i, p)| (30000 + i as u16, p))
            .collect();
        validate(kind, "urltest", &kind.build_test_config(&entries).unwrap());
    }
}

#[test]
fn dns_parsing() {
    use super::dns::DnsServer;
    let d = DnsServer::parse("https://dns.google/dns-query").unwrap();
    assert_eq!(
        (d.kind, d.server.as_str(), d.path.as_deref()),
        ("https", "dns.google", Some("/dns-query"))
    );
    let d = DnsServer::parse("8.8.8.8").unwrap();
    assert_eq!((d.kind, d.server_is_ip()), ("udp", true));
    let d = DnsServer::parse("tls://[2606:4700::1111]:853").unwrap();
    assert_eq!((d.server.as_str(), d.port), ("2606:4700::1111", Some(853)));
    assert_eq!(DnsServer::parse("local").unwrap().kind, "local");
}

#[test]
fn tun_front_config_is_valid() {
    let mut o = opts();
    o.tun = Some(TunOptions {
        interface: "rb-front".into(),
        stack: "mixed".into(),
        mtu: 9000,
    });
    let cfg = tun_front_config(&o, "127.0.0.1", 2080, &["xray".into()]).unwrap();
    validate(CoreKind::SingBox, "tun-front", &cfg);
    // Xray profile for the backend of the same session.
    validate(
        CoreKind::Xray,
        "tun-backend",
        &CoreKind::Xray.build_config(&pinned(), &opts()).unwrap(),
    );
}
