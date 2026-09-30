use serde::{Deserialize, Serialize};

pub type ProfileId = u64;
pub type GroupId = u64;

/// A single proxy server ("profile" in NekoBox terms).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    #[serde(default)]
    pub id: ProfileId,
    #[serde(default)]
    pub group: GroupId,
    pub name: String,
    pub server: String,
    pub port: u16,
    pub protocol: Protocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<Tls>,
    #[serde(default, skip_serializing_if = "Transport::is_tcp")]
    pub transport: Transport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<Latency>,
}

impl Profile {
    pub fn new(server: impl Into<String>, port: u16, protocol: Protocol) -> Self {
        Self {
            id: 0,
            group: 0,
            name: String::new(),
            server: server.into(),
            port,
            protocol,
            tls: None,
            transport: Transport::Tcp,
            latency: None,
        }
    }

    /// Display name, falling back to `server:port`.
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            self.address()
        } else {
            self.name.clone()
        }
    }

    /// `server:port`, with IPv6 addresses bracketed.
    pub fn address(&self) -> String {
        if self.server.contains(':') {
            format!("[{}]:{}", self.server, self.port)
        } else {
            format!("{}:{}", self.server, self.port)
        }
    }

    /// Short type label, e.g. "VLESS+Reality/gRPC".
    pub fn type_label(&self) -> String {
        if let Protocol::Xray { config } = &self.protocol {
            return crate::xray_json::describe(config);
        }
        let mut s = self.protocol.name().to_string();
        if let Some(tls) = &self.tls {
            if tls.reality.is_some() {
                s.push_str("+Reality");
            } else if !matches!(
                self.protocol,
                Protocol::Hysteria2 { .. } | Protocol::Tuic { .. } | Protocol::Trojan { .. }
            ) {
                s.push_str("+TLS");
            }
        }
        if !self.transport.is_tcp() {
            s.push('/');
            s.push_str(self.transport.name());
        }
        s
    }

    /// Key used to detect duplicates.
    pub fn dedup_key(&self) -> String {
        let mut clone = self.clone();
        clone.id = 0;
        clone.group = 0;
        clone.name.clear();
        clone.latency = None;
        serde_json::to_string(&clone).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Protocol {
    Vmess {
        uuid: String,
        #[serde(default)]
        alter_id: u16,
        #[serde(default = "default_vmess_security")]
        security: String,
    },
    Vless {
        uuid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flow: Option<String>,
    },
    Trojan {
        password: String,
    },
    Shadowsocks {
        method: String,
        password: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plugin: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plugin_opts: Option<String>,
    },
    Hysteria2 {
        password: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        obfs_password: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        up_mbps: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        down_mbps: Option<u32>,
    },
    Tuic {
        uuid: String,
        password: String,
        #[serde(default = "default_tuic_cc")]
        congestion_control: String,
        #[serde(default = "default_tuic_relay")]
        udp_relay_mode: String,
    },
    Socks {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<String>,
    },
    Http {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<String>,
    },
    /// A whole Xray JSON config from a subscription (Happ format), run as is
    /// except for the local inbounds. See [`crate::xray_json`].
    Xray {
        config: serde_json::Value,
    },
}

fn default_vmess_security() -> String {
    "auto".into()
}
fn default_tuic_cc() -> String {
    "bbr".into()
}
fn default_tuic_relay() -> String {
    "native".into()
}

impl Protocol {
    pub fn name(&self) -> &'static str {
        match self {
            Protocol::Vmess { .. } => "VMess",
            Protocol::Vless { .. } => "VLESS",
            Protocol::Trojan { .. } => "Trojan",
            Protocol::Shadowsocks { .. } => "Shadowsocks",
            Protocol::Hysteria2 { .. } => "Hysteria2",
            Protocol::Tuic { .. } => "TUIC",
            Protocol::Socks { .. } => "SOCKS5",
            Protocol::Http { .. } => "HTTP",
            Protocol::Xray { .. } => "Xray",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Tls {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alpn: Vec<String>,
    #[serde(default)]
    pub insecure: bool,
    /// uTLS fingerprint (chrome, firefox, safari, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reality: Option<Reality>,
    /// SHA-256 of the server certificate (lowercase hex, no colons), the `pcs`
    /// link parameter. When set, the certificate is verified by this pin instead of
    /// the CA chain (servers with self-signed certificates).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_cert_sha256: Vec<String>,
    /// SHA-256 of the pinned certificate's public key (base64). Derived at runtime
    /// from a certificate that matched `pinned_cert_sha256` (see `tls_pin`); sing-box
    /// can only pin public keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_pubkey_sha256: Vec<String>,
}

/// Normalizes a certificate hash: `AB:CD:…` / `abcd…` → `abcd…`.
pub fn normalize_cert_hash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_lowercase()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Reality {
    pub public_key: String,
    #[serde(default)]
    pub short_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spider_x: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Tcp,
    Ws {
        #[serde(default)]
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },
    Grpc {
        #[serde(default)]
        service_name: String,
    },
    /// HTTP/2 transport.
    Http {
        #[serde(default)]
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },
    HttpUpgrade {
        #[serde(default)]
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },
    Xhttp {
        #[serde(default)]
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
    },
}

impl Transport {
    pub fn is_tcp(&self) -> bool {
        matches!(self, Transport::Tcp)
    }

    pub fn name(&self) -> &'static str {
        match self {
            Transport::Tcp => "tcp",
            Transport::Ws { .. } => "ws",
            Transport::Grpc { .. } => "grpc",
            Transport::Http { .. } => "http",
            Transport::HttpUpgrade { .. } => "httpupgrade",
            Transport::Xhttp { .. } => "xhttp",
        }
    }
}

/// Result of the last latency test.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Latency {
    Ms(u32),
    Error(String),
}

impl std::fmt::Display for Latency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Latency::Ms(ms) => write!(f, "{ms} ms"),
            Latency::Error(e) => write!(f, "{e}"),
        }
    }
}

/// A group of profiles. A group with a subscription is refreshed from its URL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<Subscription>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Subscription {
    pub url: String,
    /// Overrides the global subscription User-Agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// Unix timestamp of the last successful update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<SubscriptionInfo>,
}

/// Parsed `subscription-userinfo` header.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SubscriptionInfo {
    pub upload: u64,
    pub download: u64,
    pub total: u64,
    /// Unix timestamp, 0 = never.
    pub expire: u64,
}
