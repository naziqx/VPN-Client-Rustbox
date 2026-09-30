//! Share links: `vmess://`, `vless://`, `trojan://`, `ss://`, `hysteria2://` (`hy2://`),
//! `tuic://`, `socks://` (`socks5://`).

mod export;
mod parse;
mod uri;

pub use export::to_link;
pub use parse::parse_link;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};

use crate::model::Profile;

#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("unsupported scheme: {0}")]
    UnsupportedScheme(String),
    #[error("malformed link: {0}")]
    Malformed(String),
}

/// Parses every recognizable link in `text` (one per line).
/// Returns parsed profiles and errors for lines that looked like links but failed.
pub fn parse_many(text: &str) -> (Vec<Profile>, Vec<String>) {
    let mut profiles = Vec::new();
    let mut errors = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| l.contains("://")) {
        match parse_link(line) {
            Ok(p) => profiles.push(p),
            Err(e) => errors.push(format!("{e}: {}", truncate(line, 60))),
        }
    }
    (profiles, errors)
}

/// Lenient base64 decoding: standard/url-safe alphabet, with or without padding,
/// ignoring whitespace.
pub fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    for engine in [&STANDARD, &URL_SAFE, &STANDARD_NO_PAD, &URL_SAFE_NO_PAD] {
        if let Ok(bytes) = engine.decode(&cleaned) {
            return Some(bytes);
        }
    }
    // Some providers emit wrong padding.
    let trimmed = cleaned.trim_end_matches('=');
    [&STANDARD_NO_PAD, &URL_SAFE_NO_PAD]
        .into_iter()
        .find_map(|engine| engine.decode(trimmed).ok())
}

pub fn decode_base64_str(input: &str) -> Option<String> {
    decode_base64(input).and_then(|b| String::from_utf8(b).ok())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests;
