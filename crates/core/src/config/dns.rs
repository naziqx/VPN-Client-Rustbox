//! Parsing of user-facing DNS server strings.

#[derive(Debug, Clone, PartialEq)]
pub struct DnsServer {
    /// local, udp, tcp, tls, https, h3, quic
    pub kind: &'static str,
    pub server: String,
    pub port: Option<u16>,
    pub path: Option<String>,
}

impl DnsServer {
    pub fn parse(spec: &str) -> anyhow::Result<Self> {
        let spec = spec.trim();
        if spec.is_empty() || spec.eq_ignore_ascii_case("local") || spec == "localhost" {
            return Ok(Self {
                kind: "local",
                server: String::new(),
                port: None,
                path: None,
            });
        }
        let (kind, rest) = match spec.split_once("://") {
            Some((scheme, rest)) => {
                let kind = match scheme.to_ascii_lowercase().as_str() {
                    "udp" => "udp",
                    "tcp" => "tcp",
                    "tls" => "tls",
                    "https" => "https",
                    "h3" => "h3",
                    "quic" => "quic",
                    other => anyhow::bail!("unsupported DNS scheme '{other}'"),
                };
                (kind, rest)
            }
            None => ("udp", spec),
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], Some(rest[i..].to_string())),
            None => (rest, None),
        };
        let (server, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (host, after) = v6.split_once(']').unwrap_or((v6, ""));
            (
                host.to_string(),
                after.strip_prefix(':').and_then(|p| p.parse().ok()),
            )
        } else if authority.matches(':').count() == 1 {
            let (h, p) = authority.split_once(':').unwrap();
            (h.to_string(), p.parse().ok())
        } else {
            (authority.to_string(), None)
        };
        anyhow::ensure!(!server.is_empty(), "empty DNS server in '{spec}'");
        Ok(Self {
            kind,
            server,
            port,
            path: path.filter(|p| p != "/"),
        })
    }

    pub fn server_is_ip(&self) -> bool {
        self.server.parse::<std::net::IpAddr>().is_ok()
    }

    /// Canonical URL form, used by Xray.
    pub fn to_url(&self) -> String {
        if self.kind == "local" {
            return "localhost".into();
        }
        let host = if self.server.contains(':') {
            format!("[{}]", self.server)
        } else {
            self.server.clone()
        };
        let port = self.port.map(|p| format!(":{p}")).unwrap_or_default();
        match self.kind {
            "udp" => format!("{host}{port}"),
            "https" => format!(
                "https://{host}{port}{}",
                self.path.as_deref().unwrap_or("/dns-query")
            ),
            kind => format!("{kind}://{host}{port}"),
        }
    }
}
