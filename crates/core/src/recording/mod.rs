//! Recordings: what the browser SDK sends, how it is cleaned and stored, and how it is read back.

pub mod extract;
pub mod ingest;
pub mod mask;

use std::io::{Read, Write};
use std::net::IpAddr;

use chrono::{DateTime, Utc};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use opentrail_crypto::{DataKey, Keys, open_blob, seal_blob};
use uuid::Uuid;

use crate::error::{AppError, Result};

/// The prefix of every public ingest key. The key is in page source, so it is not a secret.
pub const INGEST_KEY_PREFIX: &str = "trk_pub_";

pub fn project_key(keys: &Keys, project_id: Uuid, wrapped: &str) -> Result<DataKey> {
    Ok(keys.unwrap_data_key(wrapped, &project_id.to_string())?)
}

fn chunk_aad(session_id: Uuid, window_id: Uuid, seq: i32) -> Vec<u8> {
    format!("chunk:{session_id}:{window_id}:{seq}").into_bytes()
}

fn body_aad(session_id: Uuid, ts: DateTime<Utc>, which: &str) -> Vec<u8> {
    format!("body:{session_id}:{}:{which}", ts.timestamp_micros()).into_bytes()
}

pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(data.len() / 4), Compression::new(6));
    // Writing to a Vec cannot fail.
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

/// Decompresses at most `limit` bytes. `None` when the data is not gzip or is larger than that.
pub fn gunzip(data: &[u8], limit: u64) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let read = GzDecoder::new(data).take(limit + 1).read_to_end(&mut out).ok()?;
    (read as u64 <= limit).then_some(out)
}

/// A batch's events, compressed and sealed under the project's key for this one chunk.
pub fn seal_chunk(key: &DataKey, session_id: Uuid, window_id: Uuid, seq: i32, events_json: &[u8]) -> Vec<u8> {
    seal_blob(key, &chunk_aad(session_id, window_id, seq), &gzip(events_json))
}

pub fn open_chunk(key: &DataKey, session_id: Uuid, window_id: Uuid, seq: i32, sealed: &[u8]) -> Result<Vec<u8>> {
    let compressed = open_blob(key, &chunk_aad(session_id, window_id, seq), sealed)?;
    gunzip(&compressed, 64 * 1024 * 1024).ok_or_else(|| AppError::internal(anyhow::anyhow!("a stored chunk is not gzip")))
}

pub fn seal_body(key: &DataKey, session_id: Uuid, ts: DateTime<Utc>, which: &str, body: &str) -> Vec<u8> {
    seal_blob(key, &body_aad(session_id, ts, which), body.as_bytes())
}

pub fn open_body(key: &DataKey, session_id: Uuid, ts: DateTime<Utc>, which: &str, sealed: &[u8]) -> Result<String> {
    let plain = open_blob(key, &body_aad(session_id, ts, which), sealed)?;
    Ok(String::from_utf8_lossy(&plain).into_owned())
}

/// The /24 (IPv4) or /48 (IPv6) the visitor came from. Enough to tell offices apart, not people.
pub fn ip_prefix(ip: IpAddr) -> String {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            format!("{a}.{b}.{c}.0/24")
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}::/48", s[0], s[1], s[2])
        }
    }
}

/// Browser and operating system with the browser's major version, from a User-Agent string.
pub fn browser_and_os(user_agent: &str) -> (Option<String>, Option<String>) {
    let version = |marker: &str| -> Option<String> {
        let rest = &user_agent[user_agent.find(marker)? + marker.len()..];
        let major: String = rest.chars().take_while(char::is_ascii_digit).collect();
        (!major.is_empty()).then_some(major)
    };
    let named = |name: &str, marker: &str| version(marker).map(|v| format!("{name} {v}")).or_else(|| Some(name.to_string()));
    let browser = if user_agent.contains("Edg/") {
        named("Edge", "Edg/")
    } else if user_agent.contains("OPR/") {
        named("Opera", "OPR/")
    } else if user_agent.contains("Firefox/") {
        named("Firefox", "Firefox/")
    } else if user_agent.contains("Chrome/") {
        named("Chrome", "Chrome/")
    } else if user_agent.contains("Safari/") {
        named("Safari", "Version/")
    } else {
        None
    };
    let os = [
        ("Windows", "Windows"),
        ("Android", "Android"),
        ("iPhone", "iOS"),
        ("iPad", "iOS"),
        ("CrOS", "ChromeOS"),
        ("Mac OS X", "macOS"),
        ("Linux", "Linux"),
    ]
    .into_iter()
    .find(|(marker, _)| user_agent.contains(marker))
    .map(|(_, name)| name.to_string());
    (browser, os)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agents() {
        let chrome = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
        assert_eq!(browser_and_os(chrome), (Some("Chrome 142".into()), Some("Windows".into())));
        let safari = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";
        assert_eq!(browser_and_os(safari), (Some("Safari 18".into()), Some("iOS".into())));
        assert_eq!(browser_and_os("curl/8"), (None, None));
    }

    #[test]
    fn prefixes() {
        assert_eq!(ip_prefix("203.0.113.77".parse().unwrap_or_else(|_| unreachable!())), "203.0.113.0/24");
        assert_eq!(ip_prefix("2001:db8:1:2::9".parse().unwrap_or_else(|_| unreachable!())), "2001:db8:1::/48");
    }

    #[test]
    fn chunks_round_trip_and_stay_bound() {
        let key = DataKey::generate();
        let (s, w) = (Uuid::now_v7(), Uuid::now_v7());
        let sealed = seal_chunk(&key, s, w, 3, br#"[{"type":4}]"#);
        assert_eq!(open_chunk(&key, s, w, 3, &sealed).ok().as_deref(), Some(&br#"[{"type":4}]"#[..]));
        assert!(open_chunk(&key, s, w, 4, &sealed).is_err(), "another sequence number");
        assert!(gunzip(&gzip(&[0u8; 2000]), 1000).is_none(), "over the limit");
    }
}
