use super::*;
use crate::model::{Protocol, Transport};
use base64::Engine;

const UUID: &str = "b831381d-6324-4d53-ad4f-8cda48b30811";

#[test]
fn vless_reality_grpc() {
    let link = format!(
        "vless://{UUID}@example.com:443?encryption=none&security=reality&sni=www.microsoft.com&fp=chrome\
         &pbk=Z84J2IelR9ch3k8VtlVhhs5ycBUlXA7wHBWcBrjqnAw&sid=6ba85179e30d4fc2&type=grpc&serviceName=svc\
         &flow=xtls-rprx-vision#%F0%9F%87%A9%F0%9F%87%AA%20Germany"
    );
    let p = parse_link(&link).unwrap();
    assert_eq!(p.name, "🇩🇪 Germany");
    assert_eq!(p.server, "example.com");
    assert_eq!(p.port, 443);
    assert_eq!(
        p.protocol,
        Protocol::Vless {
            uuid: UUID.into(),
            flow: Some("xtls-rprx-vision".into())
        }
    );
    let tls = p.tls.as_ref().unwrap();
    assert_eq!(tls.sni.as_deref(), Some("www.microsoft.com"));
    assert_eq!(tls.reality.as_ref().unwrap().short_id, "6ba85179e30d4fc2");
    assert_eq!(
        p.transport,
        Transport::Grpc {
            service_name: "svc".into()
        }
    );
    assert_eq!(p.type_label(), "VLESS+Reality/grpc");
}

#[test]
fn vmess_json() {
    let json = format!(
        r#"{{"v":"2","ps":"test","add":"1.2.3.4","port":8443,"id":"{UUID}","aid":"0","net":"ws","type":"none","host":"cdn.example.com","path":"/ws?ed=2048","tls":"tls","sni":"","alpn":"h2,http/1.1"}}"#
    );
    let link = format!(
        "vmess://{}",
        base64::engine::general_purpose::STANDARD.encode(json)
    );
    let p = parse_link(&link).unwrap();
    assert_eq!(p.port, 8443);
    assert_eq!(p.name, "test");
    assert_eq!(
        p.transport,
        Transport::Ws {
            path: "/ws?ed=2048".into(),
            host: Some("cdn.example.com".into())
        }
    );
    let tls = p.tls.unwrap();
    assert_eq!(tls.sni.as_deref(), Some("cdn.example.com"));
    assert_eq!(tls.alpn, vec!["h2", "http/1.1"]);
}

#[test]
fn shadowsocks_forms() {
    // SIP002 with base64 userinfo
    let p = parse_link("ss://YWVzLTI1Ni1nY206cGFzcw@1.2.3.4:8388#name").unwrap();
    assert_eq!(
        p.protocol,
        Protocol::Shadowsocks {
            method: "aes-256-gcm".into(),
            password: "pass".into(),
            plugin: None,
            plugin_opts: None
        }
    );
    // plain userinfo (2022 ciphers)
    let p = parse_link("ss://2022-blake3-aes-128-gcm:a2V5%3D@[2001:db8::1]:443#v6").unwrap();
    assert_eq!(p.server, "2001:db8::1");
    assert!(matches!(&p.protocol, Protocol::Shadowsocks { password, .. } if password == "a2V5="));
    // legacy full base64
    let legacy = base64::engine::general_purpose::STANDARD
        .encode("chacha20-ietf-poly1305:pw@example.org:1234");
    let p = parse_link(&format!("ss://{legacy}#old")).unwrap();
    assert_eq!((p.server.as_str(), p.port), ("example.org", 1234));
    // plugin
    let p = parse_link("ss://YWVzLTI1Ni1nY206cGFzcw@1.2.3.4:8388/?plugin=obfs-local%3Bobfs%3Dhttp%3Bobfs-host%3Da.com#p").unwrap();
    assert!(
        matches!(&p.protocol, Protocol::Shadowsocks { plugin: Some(pl), plugin_opts: Some(o), .. }
        if pl == "obfs-local" && o == "obfs=http;obfs-host=a.com")
    );
}

#[test]
fn hysteria2_port_hopping() {
    let p = parse_link("hy2://secret@hy.example.com:443,20000-30000/?sni=hy.example.com&obfs=salamander&obfs-password=ob&insecure=1#HY").unwrap();
    assert_eq!(p.port, 443);
    assert!(p.tls.as_ref().unwrap().insecure);
    assert!(matches!(&p.protocol, Protocol::Hysteria2 { obfs_password: Some(o), .. } if o == "ob"));
}

#[test]
fn tuic_and_trojan_and_socks() {
    let p = parse_link(&format!(
        "tuic://{UUID}:pw@t.example.com:443?congestion_control=cubic&alpn=h3&sni=t.example.com#T"
    ))
    .unwrap();
    assert!(
        matches!(&p.protocol, Protocol::Tuic { congestion_control, .. } if congestion_control == "cubic")
    );

    let p =
        parse_link("trojan://p%40ss@tr.example.com:443?type=ws&path=%2Fws&host=h.com#Tr").unwrap();
    assert!(matches!(&p.protocol, Protocol::Trojan { password } if password == "p@ss"));
    assert!(p.tls.is_some(), "trojan defaults to TLS");

    let p = parse_link("socks://dXNlcjpwYXNz@127.0.0.1:1080#S").unwrap();
    assert_eq!(
        p.protocol,
        Protocol::Socks {
            username: Some("user".into()),
            password: Some("pass".into())
        }
    );
}

#[test]
fn roundtrip_all() {
    let links = [
        format!(
            "vless://{UUID}@example.com:443?security=reality&sni=a.com&fp=chrome&pbk=KEY&sid=ab&type=tcp&flow=xtls-rprx-vision#A"
        ),
        format!(
            "vless://{UUID}@example.com:443?security=tls&sni=a.com&type=xhttp&path=%2Fx&mode=auto#X"
        ),
        "trojan://pw@example.com:443?security=tls&sni=a.com&type=grpc&serviceName=g#B".to_string(),
        "ss://YWVzLTI1Ni1nY206cGFzcw@1.2.3.4:8388#C".to_string(),
        "hysteria2://pw@example.com:443?sni=a.com&obfs=salamander&obfs-password=o#D".to_string(),
        format!("tuic://{UUID}:pw@example.com:443?sni=a.com&alpn=h3#E"),
        "socks://dXNlcjpwYXNz@127.0.0.1:1080#F".to_string(),
    ];
    for link in links {
        let p = parse_link(&link).unwrap();
        let again = parse_link(&to_link(&p)).unwrap();
        assert_eq!(p, again, "roundtrip failed for {link}");
    }
    // vmess via JSON
    let mut p = parse_link(&links_vmess()).unwrap();
    let again = parse_link(&to_link(&p)).unwrap();
    p.tls.as_mut().unwrap().insecure = false;
    assert_eq!(p, again);
}

fn links_vmess() -> String {
    let json = format!(
        r#"{{"add":"a.com","port":"443","id":"{UUID}","net":"grpc","path":"svc","tls":"tls","ps":"V"}}"#
    );
    format!(
        "vmess://{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    )
}

#[test]
fn pinned_certificate_hash() {
    let pcs = "AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89";
    let link = format!(
        "vless://{UUID}@1.2.3.4:443?encryption=none&type=grpc&serviceName=svc&mode=gun&security=tls&sni=lk.x5.ru&fp=edge&pcs={pcs}#pinned"
    );
    let p = parse_link(&link).unwrap();
    let tls = p.tls.as_ref().unwrap();
    assert_eq!(tls.pinned_cert_sha256, vec!["abcdef0123456789".repeat(4)]);
    assert_eq!(parse_link(&to_link(&p)).unwrap(), p);
}

#[test]
fn parse_many_reports_errors() {
    let text =
        "vless://bad\nhttps://not-a-proxy.com\n\nss://YWVzLTI1Ni1nY206cGFzcw@1.2.3.4:8388#ok\n";
    let (profiles, errors) = parse_many(text);
    assert_eq!(profiles.len(), 1);
    assert_eq!(errors.len(), 2);
}
