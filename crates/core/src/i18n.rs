//! The languages the app and its emails can be shown in. English only for now.

use crate::error::{Result, bad_request};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
}

/// Languages a user can choose, as code and name.
pub const LANGUAGES: &[(&str, &str)] = &[("en", "English")];

impl Lang {
    pub const ALL: &'static [Lang] = &[Lang::En];

    /// A supported language from a code such as `en`, `en-GB` or `en_US`.
    pub fn parse(code: &str) -> Option<Self> {
        let base = code.trim().split(['-', '_']).next().unwrap_or_default().to_ascii_lowercase();
        Self::ALL.iter().copied().find(|l| l.code() == base)
    }

    pub fn of(code: &str) -> Self {
        Self::parse(code).unwrap_or(Self::En)
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
        }
    }

    pub fn rtl(self) -> bool {
        false
    }
}

/// The stored code for a language the user picked, or an error naming the codes there are.
pub fn language_code(value: &str) -> Result<String> {
    match Lang::parse(value) {
        Some(lang) => Ok(lang.code().to_string()),
        None => Err(bad_request("Use one of these language codes: {codes}")
            .params([("codes", &LANGUAGES.iter().map(|(code, _)| *code).collect::<Vec<_>>().join(", "))])),
    }
}
