//! What the server does to every recorded string, whatever the SDK already did: identity and card
//! numbers become asterisks, and nothing that could run survives (`on*` attributes, `javascript:` URLs).

use serde_json::Value;

/// Masks Swedish personal identity numbers (also coordination numbers) and payment card numbers in
/// place. Digits become `*` and separators stay, so replayed text keeps its width.
pub fn mask_text(text: &mut String) -> bool {
    let bytes = text.as_bytes();
    if !bytes.iter().any(u8::is_ascii_digit) {
        return false;
    }
    let mut out = bytes.to_vec();
    let mut changed = false;
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0 || !is_word(bytes[i - 1]);
        if !bytes[i].is_ascii_digit() || !boundary {
            i += 1;
            continue;
        }
        let (end, digits, separators) = number_run(bytes, i);
        if (end == bytes.len() || !is_word(bytes[end]))
            && (is_identity_number(&digits, &separators) || is_card_number(&digits, &separators))
        {
            for b in &mut out[i..end] {
                if b.is_ascii_digit() {
                    *b = b'*';
                }
            }
            changed = true;
        }
        i = end.max(i + 1);
    }
    if changed {
        // Only ASCII digits were replaced with an ASCII byte, so this is still UTF-8.
        *text = String::from_utf8(out).unwrap_or_default();
    }
    changed
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Digits from `start`, each separated by at most one space, `-` or `+`. Returns the end, the digits
/// and, for each separator, how many digits came before it and which byte it was.
fn number_run(bytes: &[u8], start: usize) -> (usize, Vec<u8>, Vec<(usize, u8)>) {
    let mut digits = Vec::new();
    let mut separators = Vec::new();
    let mut i = start;
    let mut end = start;
    while i < bytes.len() && digits.len() <= 19 {
        let b = bytes[i];
        if b.is_ascii_digit() {
            digits.push(b - b'0');
            i += 1;
            end = i;
        } else if matches!(b, b' ' | b'-' | b'+') && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() && end == i {
            separators.push((digits.len(), b));
            i += 1;
        } else {
            break;
        }
    }
    (end, digits, separators)
}

fn luhn(digits: &[u8]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            let d = d as u32;
            if i % 2 == 1 {
                let x = d * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// YYMMDD-NNNN or YYYYMMDD-NNNN (also with `+`, or no separator), with a real month, a day that may
/// carry the coordination number's +60, and a valid check digit.
fn is_identity_number(digits: &[u8], separators: &[(usize, u8)]) -> bool {
    let date_len = match digits.len() {
        10 => 6,
        12 => 8,
        _ => return false,
    };
    match separators {
        [] => {}
        [(at, b'-' | b'+')] if *at == date_len => {}
        _ => return false,
    }
    let ten = &digits[digits.len() - 10..];
    if date_len == 8 && !matches!((digits[0], digits[1]), (1, 8 | 9) | (2, 0)) {
        return false;
    }
    let month = ten[2] * 10 + ten[3];
    let day = ten[4] * 10 + ten[5];
    (1..=12).contains(&month) && ((1..=31).contains(&day) || (61..=91).contains(&day)) && luhn(ten)
}

/// 13 to 19 digits with a card network's leading digits and a valid check digit. The prefixes keep
/// millisecond timestamps and most ids out.
fn is_card_number(digits: &[u8], separators: &[(usize, u8)]) -> bool {
    let issuer = matches!(digits, [4, ..] | [6, ..] | [5, 1..=5, ..] | [2, 2..=7, ..] | [3, 0 | 4 | 5 | 6 | 7 | 8, ..]);
    issuer
        && (13..=19).contains(&digits.len())
        && separators.iter().all(|(_, b)| matches!(b, b' ' | b'-'))
        && separators.windows(2).all(|w| w[0].1 == w[1].1)
        && luhn(digits)
}

/// Masks every string in a recorded event and removes what could run when it is replayed.
/// Returns how many strings were masked.
pub fn sanitize(value: &mut Value) -> usize {
    match value {
        Value::String(s) => usize::from(mask_text(s)),
        Value::Array(items) => items.iter_mut().map(sanitize).sum(),
        Value::Object(map) => {
            if let Some(Value::Object(attributes)) = map.get_mut("attributes") {
                attributes.retain(|name, value| !is_handler(name) && !is_script_url(value));
            }
            map.values_mut().map(sanitize).sum()
        }
        _ => 0,
    }
}

fn is_handler(name: &str) -> bool {
    name.len() > 2 && name.as_bytes()[..2].eq_ignore_ascii_case(b"on")
}

fn is_script_url(value: &Value) -> bool {
    let Some(s) = value.as_str() else { return false };
    let compact: String = s.chars().filter(|c| !c.is_whitespace() && !c.is_control()).take(16).collect::<String>().to_ascii_lowercase();
    compact.starts_with("javascript:") || compact.starts_with("vbscript:") || compact.starts_with("data:text/html")
}

/// Query parameters whose values are credentials, masked in every recorded URL.
const SECRET_PARAMS: &[&str] = &[
    "token",
    "access_token",
    "refresh_token",
    "id_token",
    "code",
    "password",
    "pwd",
    "secret",
    "client_secret",
    "key",
    "api_key",
    "apikey",
    "auth",
    "sig",
    "signature",
    "session",
    "sessionid",
    "jwt",
];

/// The URL with credential-like query values and any user info replaced, and the fragment kept
/// only if it holds no `=` (OAuth implicit flows put tokens there).
pub fn clean_url(url: &str) -> String {
    let mut url = url.trim().chars().take(2048).collect::<String>();
    if let Some((scheme, rest)) = url.split_once("://")
        && let Some(at) = rest.find('@')
        && rest[..at].find(['/', '?', '#']).is_none()
    {
        url = format!("{scheme}://{}", &rest[at + 1..]);
    }
    let (base, fragment) = match url.split_once('#') {
        Some((b, f)) => (b.to_string(), (!f.contains('=')).then(|| f.to_string())),
        None => (url.clone(), None),
    };
    let (path, query) = match base.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (base, None),
    };
    let mut out = path;
    if let Some(query) = query {
        let cleaned: Vec<String> = query
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((name, _)) if SECRET_PARAMS.contains(&name.to_ascii_lowercase().as_str()) => format!("{name}=****"),
                _ => {
                    let mut pair = pair.to_string();
                    mask_text(&mut pair);
                    pair
                }
            })
            .collect();
        out.push('?');
        out.push_str(&cleaned.join("&"));
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(&fragment);
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn masked(s: &str) -> String {
        let mut s = s.to_string();
        mask_text(&mut s);
        s
    }

    #[test]
    fn identity_numbers_are_masked() {
        assert_eq!(masked("Pnr 19811218-9876 ok"), "Pnr ********-**** ok");
        assert_eq!(masked("811218-9876"), "******-****");
        assert_eq!(masked("8112189876"), "**********");
        assert_eq!(masked("198112189876"), "************");
        assert_eq!(masked("samordning 811278-9873"), "samordning ******-****", "day + 60");
    }

    #[test]
    fn other_numbers_are_left_alone() {
        for text in [
            "811218-9877",
            "2026-11-20 12:00",
            "070-123 45 67",
            "order 1234567890",
            "id198112189876",
            "1700000000000",
            "width: 1024px",
            "1981-12-18",
        ] {
            assert_eq!(masked(text), text, "{text}");
        }
    }

    #[test]
    fn card_numbers_are_masked() {
        assert_eq!(masked("4111 1111 1111 1111"), "**** **** **** ****");
        assert_eq!(masked("card 4111-1111-1111-1111."), "card ****-****-****-****.");
        assert_eq!(masked("5555555555554444"), "****************");
        assert_eq!(masked("4111 1111 1111 1112"), "4111 1111 1111 1112", "fails Luhn");
        assert_eq!(masked("4111 1111-1111 1111"), "4111 1111-1111 1111", "mixed separators");
    }

    #[test]
    fn handlers_and_script_urls_are_removed() {
        let mut node = json!({
            "type": 2, "tagName": "a",
            "attributes": { "href": " JavaScript:alert(1)", "onclick": "x()", "ONLOAD": "y", "class": "link", "title": "811218-9876" },
            "childNodes": [{ "type": 3, "textContent": "4111 1111 1111 1111" }]
        });
        assert_eq!(sanitize(&mut node), 2);
        assert_eq!(node["attributes"], json!({ "class": "link", "title": "******-****" }));
        assert_eq!(node["childNodes"][0]["textContent"], "**** **** **** ****");

        let mut mutation = json!({ "source": 0, "attributes": [{ "id": 4, "attributes": { "onerror": "z", "src": "data:text/html,<b>" , "alt": "ok" } }] });
        sanitize(&mut mutation);
        assert_eq!(mutation["attributes"][0]["attributes"], json!({ "alt": "ok" }));
    }

    #[test]
    fn urls_lose_credentials() {
        assert_eq!(
            clean_url("https://user:pw@shop.example/cb?code=abc&state=1&Token=x#access_token=y"),
            "https://shop.example/cb?code=****&state=1&Token=****"
        );
        assert_eq!(clean_url("https://shop.example/p?pnr=8112189876#top"), "https://shop.example/p?pnr=**********#top");
        assert_eq!(clean_url("/api/items?page=2"), "/api/items?page=2");
    }
}
