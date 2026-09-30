//! Minimal lenient URI splitter. `url::Url` rejects things that appear in the
//! wild (port ranges for hysteria2, raw base64 in userinfo), so we split by hand.

use percent_encoding::percent_decode_str;

use super::LinkError;

#[derive(Debug, Default)]
pub struct RawUri {
    /// Raw (still percent-encoded) userinfo.
    pub userinfo: Option<String>,
    pub host: String,
    /// Raw port string, may be a list/range (`443,8000-9000`).
    pub port: String,
    pub query: Vec<(String, String)>,
    pub fragment: String,
}

impl RawUri {
    pub fn parse(link: &str) -> Result<Self, LinkError> {
        let (_, rest) = link
            .split_once("://")
            .ok_or_else(|| LinkError::Malformed("missing ://".into()))?;
        let (rest, fragment) = match rest.split_once('#') {
            Some((r, f)) => (r, decode(f)),
            None => (rest, String::new()),
        };
        let (rest, query) = match rest.split_once('?') {
            Some((r, q)) => (r, parse_query(q)),
            None => (rest, Vec::new()),
        };
        let authority = rest.split('/').next().unwrap_or_default();
        let (userinfo, hostport) = match authority.rsplit_once('@') {
            Some((u, h)) => (Some(u.to_string()), h),
            None => (None, authority),
        };
        let (host, port) = split_host_port(hostport)?;
        Ok(Self {
            userinfo,
            host,
            port,
            query,
            fragment,
        })
    }

    pub fn param(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    pub fn param_any(&self, keys: &[&str]) -> Option<&str> {
        keys.iter().find_map(|k| self.param(k))
    }

    pub fn flag(&self, keys: &[&str]) -> bool {
        self.param_any(keys)
            .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
    }

    /// First port of a possibly multi-port specification.
    pub fn first_port(&self) -> Result<u16, LinkError> {
        let first = self.port.split([',', '-', ':']).next().unwrap_or_default();
        first
            .parse()
            .map_err(|_| LinkError::Malformed(format!("bad port '{}'", self.port)))
    }

    pub fn userinfo_decoded(&self) -> Option<String> {
        self.userinfo.as_deref().map(decode)
    }
}

pub fn decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    // `form_urlencoded` turns '+' into ' ', which breaks base64 values; decode manually.
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(pair), String::new()),
        })
        .collect()
}

pub fn split_host_port(hostport: &str) -> Result<(String, String), LinkError> {
    if let Some(rest) = hostport.strip_prefix('[') {
        let (host, after) = rest
            .split_once(']')
            .ok_or_else(|| LinkError::Malformed("unterminated IPv6 address".into()))?;
        let port = after.strip_prefix(':').unwrap_or_default();
        return Ok((host.to_string(), port.to_string()));
    }
    match hostport.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => Ok((decode(h), p.to_string())),
        _ => Err(LinkError::Malformed(format!(
            "missing port in '{hostport}'"
        ))),
    }
}
