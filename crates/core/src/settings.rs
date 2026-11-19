use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use opentrail_crypto::{KeyPurpose, Keys};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgConnection;

use crate::error::{AppError, Result, bad_request};

/// A group of Admin settings, stored as one row. Secret fields are encrypted one by one, bound to
/// their group and name, and never returned to a client: reads show only whether they are set.
pub trait SettingsGroup: Serialize + DeserializeOwned + Default + JsonSchema + Send + Sync {
    const NAME: &'static str;
    const SECRETS: &'static [&'static str] = &[];
    /// For each secret, the fields that decide where it is sent. Outside the app they cannot change
    /// while the secret is kept, so an agent cannot point a saved secret at another server.
    const SECRET_TARGETS: &'static [(&'static str, &'static [&'static str])] = &[];
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct OrgSettings {
    pub name: String,
    pub time_zone: String,
    pub locale: String,
}

impl Default for OrgSettings {
    fn default() -> Self {
        Self { name: String::new(), time_zone: "Europe/Stockholm".into(), locale: "en".into() }
    }
}

impl SettingsGroup for OrgSettings {
    const NAME: &'static str = "org";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct EmailSettings {
    /// `resend` or `smtp`.
    pub provider: String,
    pub api_key: Option<String>,
    pub smtp_host: String,
    pub smtp_port: u16,
    /// `starttls` (usually port 587) or `tls` (usually port 465). Plain-text SMTP is not offered.
    pub smtp_security: String,
    pub smtp_username: String,
    pub smtp_password: Option<String>,
    pub from_address: String,
    pub from_name: String,
    pub reply_to: String,
    pub verified_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for EmailSettings {
    fn default() -> Self {
        Self {
            provider: "resend".into(),
            api_key: None,
            smtp_host: String::new(),
            smtp_port: 587,
            smtp_security: "starttls".into(),
            smtp_username: String::new(),
            smtp_password: None,
            from_address: String::new(),
            from_name: String::new(),
            reply_to: String::new(),
            verified_at: None,
        }
    }
}

impl SettingsGroup for EmailSettings {
    const NAME: &'static str = "email";
    const SECRETS: &'static [&'static str] = &["apiKey", "smtpPassword"];
    const SECRET_TARGETS: &'static [(&'static str, &'static [&'static str])] =
        &[("apiKey", &["provider"]), ("smtpPassword", &["provider", "smtpHost", "smtpPort", "smtpSecurity", "smtpUsername"])];
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct SecuritySettings {
    pub enforce_two_factor: bool,
    pub session_hours: u32,
    pub idle_minutes: u32,
    /// Who may state the client's address in `X-Forwarded-For`: `private` (any proxy on a private
    /// network), `none`, or a list of addresses and CIDR ranges.
    pub trusted_proxies: String,
    /// Lets new MCP clients register themselves. Clients that already registered keep working.
    pub allow_client_registration: bool,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            enforce_two_factor: false,
            session_hours: 12,
            idle_minutes: 60,
            trusted_proxies: "private".into(),
            allow_client_registration: true,
        }
    }
}

impl SettingsGroup for SecuritySettings {
    const NAME: &'static str = "security";
}

impl SecuritySettings {
    /// A value that cannot be read, which only a pinned variable can hold, believes no proxy.
    pub fn trusted_proxies(&self) -> TrustedProxies {
        TrustedProxies::parse(&self.trusted_proxies).unwrap_or(TrustedProxies::None)
    }
}

/// Who may state the client's address for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustedProxies {
    None,
    Private,
    List(Vec<ipnet::IpNet>),
}

impl TrustedProxies {
    pub fn parse(text: &str) -> Result<Self> {
        let entries: Vec<String> = text.split([',', ' ', '\n']).map(str::trim).filter(|e| !e.is_empty()).map(str::to_string).collect();
        match entries.as_slice() {
            [] => Err(bad_request("Enter private, none, or the addresses of your proxies")),
            [one] if one.eq_ignore_ascii_case("private") => Ok(Self::Private),
            [one] if one.eq_ignore_ascii_case("none") => Ok(Self::None),
            _ => {
                let nets = crate::auth::normalize_allowed_ips(&entries)
                    .map_err(|_| bad_request("Enter private, none, or a list of addresses and CIDR ranges"))?;
                Ok(Self::List(nets.iter().filter_map(|n| crate::auth::parse_net(n)).collect()))
            }
        }
    }
}

/// `scheme://host[:port]` for an https origin (or http on localhost, for development), or `None`
/// when the text is anything more or less than an origin.
pub fn canonical_origin(text: &str) -> Option<String> {
    let text = text.trim().trim_end_matches('/');
    if text.contains('*') || text.contains('@') {
        return None;
    }
    let url = reqwest::Url::parse(text).ok()?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    let scheme_ok = url.scheme() == "https" || (url.scheme() == "http" && local);
    let bare = url.path() == "/" && url.query().is_none() && url.fragment().is_none();
    (scheme_ok && bare && url.host().is_some()).then(|| url.origin().ascii_serialization())
}

/// Release checks and in-app updates.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateSettings {
    /// Ask GitHub for the latest release (at most every six hours, only the version is sent).
    pub check_for_updates: bool,
    /// A secret URL that redeploys the service when called, such as Sliplane's deploy hook.
    pub deploy_hook_url: Option<String>,
    /// A page that updates this installation to a release given as `ref`, such as the update link
    /// the Sliplane Deployer passes in.
    pub update_url: Option<String>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { check_for_updates: true, deploy_hook_url: None, update_url: None }
    }
}

impl SettingsGroup for UpdateSettings {
    const NAME: &'static str = "updates";
    const SECRETS: &'static [&'static str] = &["deployHookUrl"];
}

/// Sign-in through one OpenID Connect provider.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct SsoSettings {
    pub enabled: bool,
    /// The sign-in button's text, such as `Sign In with Acme`. Empty uses a general one.
    pub button_label: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    /// The role of an account created at first sign-in. Never admin.
    pub default_role: String,
    /// Refuses a sign-in unless the provider's ID token says a second factor was used.
    pub require_provider_mfa: bool,
    /// `all`, or `adminsOnly` to keep passwords and passkeys only for admins while single sign-on is on.
    pub password_sign_in: String,
}

impl Default for SsoSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            button_label: String::new(),
            issuer: String::new(),
            client_id: String::new(),
            client_secret: None,
            default_role: "viewer".into(),
            require_provider_mfa: true,
            password_sign_in: "all".into(),
        }
    }
}

impl SettingsGroup for SsoSettings {
    const NAME: &'static str = "sso";
    const SECRETS: &'static [&'static str] = &["clientSecret"];
    const SECRET_TARGETS: &'static [(&'static str, &'static [&'static str])] = &[("clientSecret", &["issuer", "clientId"])];
}

impl SsoSettings {
    /// Switched on and has what it needs.
    pub fn ready(&self) -> bool {
        self.enabled
            && !self.issuer.trim().is_empty()
            && !self.client_id.trim().is_empty()
            && self.client_secret.as_deref().is_some_and(|s| !s.is_empty())
    }

    /// Only counts while people have another way in, so turning single sign-on off locks nobody out.
    pub fn admins_only_passwords(&self) -> bool {
        self.ready() && self.password_sign_in != "all"
    }
}

const TTL: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct SettingsCache {
    entries: RwLock<HashMap<&'static str, (Instant, Value)>>,
    pins: HashMap<String, String>,
}

fn env_name(group: &str, field: &str) -> String {
    let mut out = format!("OPENTRAIL_{}_", group.to_uppercase());
    for c in field.chars() {
        if c.is_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

fn context(group: &str, field: &str) -> String {
    format!("settings:{group}.{field}")
}

impl SettingsCache {
    /// `OPENTRAIL_<GROUP>_<FIELD>` variables pin a setting; pinned fields cannot be changed in the UI.
    /// An empty variable pins nothing: a deployer leaves a skipped optional addon's variables empty,
    /// and the admin must still be able to fill those settings in.
    pub fn from_env(vars: impl Iterator<Item = (String, String)>) -> Self {
        let pins = vars.filter(|(k, v)| k.starts_with("OPENTRAIL_") && k != "OPENTRAIL_VERSION" && !v.trim().is_empty()).collect();
        Self { entries: RwLock::default(), pins }
    }

    pub fn pinned<G: SettingsGroup>(&self) -> Vec<String> {
        let defaults = serde_json::to_value(G::default()).unwrap_or_default();
        let Value::Object(fields) = defaults else { return Vec::new() };
        fields.keys().filter(|f| self.pins.contains_key(&env_name(G::NAME, f))).cloned().collect()
    }

    fn apply_pins<G: SettingsGroup>(&self, values: &mut Map<String, Value>) {
        let defaults = serde_json::to_value(G::default()).unwrap_or_default();
        let Value::Object(fields) = defaults else { return };
        for (field, default) in fields {
            if let Some(raw) = self.pins.get(&env_name(G::NAME, &field)) {
                let value = match default {
                    Value::Bool(_) => Value::Bool(matches!(raw.as_str(), "true" | "1" | "yes")),
                    Value::Number(_) => raw
                        .parse::<i64>()
                        .map(Value::from)
                        .ok()
                        .or_else(|| raw.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number))
                        .unwrap_or(default),
                    Value::Array(_) => {
                        Value::Array(raw.split([',', ' ', '\n']).filter(|s| !s.is_empty()).map(|s| Value::String(s.into())).collect())
                    }
                    _ => Value::String(raw.clone()),
                };
                values.insert(field, value);
            }
        }
    }

    pub fn invalidate(&self, group: &str) {
        if let Ok(mut entries) = self.entries.write() {
            entries.retain(|k, _| *k != group);
        }
    }

    pub async fn get<G: SettingsGroup>(&self, conn: &mut PgConnection, keys: &Keys) -> Result<G> {
        if let Some((at, value)) = self.entries.read().ok().and_then(|e| e.get(G::NAME).cloned())
            && at.elapsed() < TTL
        {
            return serde_json::from_value(value).map_err(AppError::internal);
        }
        let row: Option<(Value, Value)> =
            sqlx::query_as(r#"select "values", secrets from settings where "group" = $1"#).bind(G::NAME).fetch_optional(&mut *conn).await?;
        let mut values = match row.as_ref().map(|r| &r.0) {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        if let Some((_, Value::Object(secrets))) = &row {
            for (field, sealed) in secrets {
                if let Some(sealed) = sealed.as_str() {
                    let plain = keys.decrypt_str(KeyPurpose::Settings, &context(G::NAME, field), sealed)?;
                    values.insert(field.clone(), Value::String(plain.to_string()));
                }
            }
        }
        self.apply_pins::<G>(&mut values);
        let merged = Value::Object(values);
        let group: G = serde_json::from_value(merged.clone())
            .map_err(|e| AppError::internal(anyhow::anyhow!("settings group {} is invalid (check OPENTRAIL_ pins): {e}", G::NAME)))?;
        if let Ok(mut entries) = self.entries.write() {
            entries.insert(G::NAME, (Instant::now(), serde_json::to_value(&group).unwrap_or(merged)));
        }
        Ok(group)
    }

    /// Saves a whole group. A secret sent as `None` keeps its stored value; send an empty string to clear it.
    pub async fn save<G: SettingsGroup>(
        &self,
        conn: &mut PgConnection,
        keys: &Keys,
        group: &G,
        updated_by: Option<uuid::Uuid>,
    ) -> Result<()> {
        let pinned = self.pinned::<G>();
        let Value::Object(mut values) = serde_json::to_value(group).map_err(AppError::internal)? else {
            return Err(bad_request("Settings must be an object"));
        };
        let current: Option<Value> =
            sqlx::query_scalar(r#"select secrets from settings where "group" = $1"#).bind(G::NAME).fetch_optional(&mut *conn).await?;
        let mut secrets = match current {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        };
        for field in G::SECRETS {
            match values.remove(*field) {
                Some(Value::String(s)) if s.is_empty() => {
                    secrets.remove(*field);
                }
                Some(Value::String(s)) => {
                    secrets.insert(field.to_string(), Value::String(keys.encrypt_str(KeyPurpose::Settings, &context(G::NAME, field), &s)));
                }
                _ => {}
            }
        }
        for field in &pinned {
            values.remove(field);
            secrets.remove(field);
        }
        sqlx::query(
            r#"insert into settings ("group", "values", secrets, updated_by, updated_at) values ($1, $2, $3, $4, now())
               on conflict ("group") do update set "values" = excluded."values", secrets = excluded.secrets, updated_by = excluded.updated_by, updated_at = now()"#,
        )
        .bind(G::NAME)
        .bind(Value::Object(values))
        .bind(Value::Object(secrets))
        .bind(updated_by)
        .execute(&mut *conn)
        .await?;
        self.invalidate(G::NAME);
        Ok(())
    }
}

/// The group as shown to a client: secrets replaced by whether they are set.
pub fn public_view<G: SettingsGroup>(group: &G) -> Value {
    let mut value = serde_json::to_value(group).unwrap_or_default();
    if let Value::Object(map) = &mut value {
        for field in G::SECRETS {
            let set = map.get(*field).is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()));
            map.insert(field.to_string(), Value::Bool(set));
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_names_follow_the_pattern() {
        assert_eq!(env_name("email", "apiKey"), "OPENTRAIL_EMAIL_API_KEY");
        assert_eq!(env_name("security", "enforceTwoFactor"), "OPENTRAIL_SECURITY_ENFORCE_TWO_FACTOR");
    }

    #[test]
    fn pins_override_and_are_listed() {
        let cache = SettingsCache::from_env(
            [
                ("OPENTRAIL_SECURITY_ENFORCE_TWO_FACTOR".to_string(), "true".to_string()),
                ("OPENTRAIL_SECURITY_SESSION_HOURS".into(), "4".into()),
                ("OPENTRAIL_SECURITY_IDLE_MINUTES".into(), "".into()),
            ]
            .into_iter(),
        );
        let mut values = Map::new();
        cache.apply_pins::<SecuritySettings>(&mut values);
        let s: SecuritySettings = serde_json::from_value(Value::Object(values)).expect("valid");
        assert!(s.enforce_two_factor);
        assert_eq!(s.session_hours, 4);
        let mut pinned = cache.pinned::<SecuritySettings>();
        pinned.sort();
        assert_eq!(pinned, ["enforceTwoFactor", "sessionHours"]);
    }

    #[test]
    fn public_view_hides_secrets() {
        let s = EmailSettings { api_key: Some("re_secret".into()), from_address: "a@b.c".into(), ..Default::default() };
        let v = public_view(&s);
        assert_eq!(v["apiKey"], Value::Bool(true));
        assert_eq!(v["fromAddress"], "a@b.c");
    }

    #[test]
    fn origins_are_exact() {
        assert_eq!(canonical_origin("https://App.Example.com/"), Some("https://app.example.com".into()));
        assert_eq!(canonical_origin("https://app.example.com:8443"), Some("https://app.example.com:8443".into()));
        assert_eq!(canonical_origin("http://localhost:3000"), Some("http://localhost:3000".into()));
        for bad in [
            "http://app.example.com",
            "https://*.example.com",
            "https://app.example.com/path",
            "https://app.example.com?x=1",
            "https://user@app.example.com",
            "javascript:alert(1)",
            "https://a.example.com https://b.example.com",
            "app.example.com",
            "",
        ] {
            assert_eq!(canonical_origin(bad), None, "{bad}");
        }
    }
}
