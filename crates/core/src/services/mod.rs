pub mod admin;
pub mod auth;
pub mod oauth;
pub mod passkeys;
pub mod projects;
pub mod sessions;
pub mod setup;
pub mod sso;
pub mod system;
pub mod users;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::context::Ctx;
use crate::error::Result;
use crate::service::{DynService, erase};
use crate::settings::SettingsGroup;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct Empty {}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Ok_ {
    pub ok: bool,
}

impl Default for Ok_ {
    fn default() -> Self {
        Self { ok: true }
    }
}

pub async fn settings<G: SettingsGroup>(ctx: &Ctx, conn: &mut PgConnection) -> Result<G> {
    ctx.app.settings.get::<G>(conn, ctx.keys()).await
}

/// Every service. MCP, REST and the RPC endpoint expose these by name.
pub fn registry() -> Vec<Box<dyn DynService>> {
    vec![
        erase::<auth::Login>(),
        erase::<auth::VerifyTwoFactor>(),
        erase::<auth::Logout>(),
        erase::<auth::WhoAmI>(),
        erase::<auth::SetLanguage>(),
        erase::<auth::BeginTwoFactor>(),
        erase::<auth::ConfirmTwoFactor>(),
        erase::<auth::ChangePassword>(),
        erase::<sso::Status>(),
        erase::<sso::Start>(),
        erase::<auth::RequestPasswordReset>(),
        erase::<auth::ResetPassword>(),
        erase::<setup::Status>(),
        erase::<setup::Complete>(),
        erase::<users::List>(),
        erase::<users::Invite>(),
        erase::<users::AcceptInvite>(),
        erase::<users::Update>(),
        erase::<users::ChangeEmail>(),
        erase::<users::ConfirmEmailChange>(),
        erase::<users::ResetTwoFactor>(),
        erase::<users::ListInvites>(),
        erase::<users::RevokeInvite>(),
        erase::<users::CreateApiKey>(),
        erase::<users::ListApiKeys>(),
        erase::<users::UpdateApiKey>(),
        erase::<users::RevokeApiKey>(),
        erase::<admin::GetSettings>(),
        erase::<admin::UpdateSettings>(),
        erase::<admin::SendTestEmail>(),
        erase::<admin::ListAudit>(),
        erase::<admin::VerifyAudit>(),
        erase::<system::GetVersion>(),
        erase::<system::Status>(),
        erase::<system::Update>(),
        erase::<passkeys::BeginRegistration>(),
        erase::<passkeys::FinishRegistration>(),
        erase::<passkeys::List>(),
        erase::<passkeys::Remove>(),
        erase::<passkeys::BeginSignIn>(),
        erase::<passkeys::SignIn>(),
        erase::<passkeys::BeginStepUp>(),
        erase::<passkeys::StepUp>(),
        erase::<oauth::Describe>(),
        erase::<oauth::Approve>(),
        erase::<oauth::Deny>(),
        erase::<oauth::ListConnections>(),
        erase::<oauth::RevokeConnection>(),
        erase::<projects::List>(),
        erase::<projects::Get>(),
        erase::<projects::Create>(),
        erase::<projects::Update>(),
        erase::<projects::Archive>(),
        erase::<projects::CreateKey>(),
        erase::<projects::RevokeKey>(),
        erase::<sessions::List>(),
        erase::<sessions::Get>(),
        erase::<sessions::GetTimeline>(),
        erase::<sessions::GetReplay>(),
        erase::<sessions::RequestBody>(),
        erase::<sessions::Delete>(),
        erase::<sessions::DeleteUser>(),
        erase::<sessions::Search>(),
        erase::<sessions::GroupEvents>(),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};

    use super::*;
    use crate::context::{Actor, Credential};
    use crate::permissions::Role;
    use crate::service::{Access, Exposure, Kind, allows};

    /// Signed-in services agents never get. Changing this list is a security decision: say why in
    /// the service's `not_a_tool` reason and in README.md.
    const HIDDEN: &[&str] = &[
        "account.change_password",
        "account.mcp_connections",
        "account.passkey_add",
        "account.passkey_begin",
        "account.passkey_remove",
        "account.passkeys",
        "account.revoke_mcp_connection",
        "account.step_up",
        "account.step_up_begin",
        "account.two_factor_begin",
        "account.two_factor_confirm",
        "api_keys.create",
        "auth.logout",
        "oauth.approve",
        "oauth.describe",
        "oauth.deny",
        "sessions.replay",
        "users.reset_two_factor",
    ];

    /// Calls that only change the caller's own preferences, allowed to read-only roles.
    const PERSONAL: &[&str] = &["account.set_language"];

    #[test]
    fn names_are_unique_dotted_and_described() {
        let mut seen = HashSet::new();
        for s in registry() {
            let meta = s.meta();
            assert!(seen.insert(meta.name), "duplicate {}", meta.name);
            assert!(meta.name.contains('.') && meta.name.chars().all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'), "{}", meta.name);
            assert!(meta.description.ends_with('.'), "{} needs a full-sentence description", meta.name);
            let schema = serde_json::to_value(s.input_schema()).expect("schema");
            assert_eq!(schema["type"], "object", "{} input must be an object", meta.name);
        }
    }

    #[test]
    fn every_service_decides_whether_it_is_a_tool() {
        let mut hidden = BTreeSet::new();
        for s in registry() {
            let meta = s.meta();
            match meta.exposure {
                Exposure::Undecided => panic!("{} must call .tool() or .not_a_tool(reason)", meta.name),
                Exposure::Hidden(reason) => {
                    assert!(reason.ends_with('.'), "{} needs a full-sentence reason", meta.name);
                    if meta.access != Access::Public {
                        hidden.insert(meta.name);
                    }
                }
                Exposure::Tool => assert_ne!(meta.access, Access::Public, "public service {} must not be a tool", meta.name),
            }
            if meta.kind == Kind::Query {
                assert!(!meta.destructive, "{} is a query and cannot be destructive", meta.name);
            }
        }
        assert_eq!(hidden, HIDDEN.iter().copied().collect(), "the hidden list changed; update HIDDEN and README.md");
    }

    fn actor(role: Role) -> Actor {
        Actor {
            user_id: uuid::Uuid::nil(),
            email: "a@example.com".into(),
            name: "A".into(),
            role,
            permissions: role.permissions(),
            must_enable_two_factor: false,
            credential: Credential::OAuth,
        }
    }

    fn tools(role: Role) -> BTreeSet<&'static str> {
        let actor = actor(role);
        registry().iter().map(|s| *s.meta()).filter(|m| m.is_tool() && allows(m.access, Some(&actor))).map(|m| m.name).collect()
    }

    #[test]
    fn role_matrix() {
        let all: BTreeSet<&str> = registry().iter().map(|s| *s.meta()).filter(|m| m.is_tool()).map(|m| m.name).collect();
        assert_eq!(tools(Role::Admin), all, "an admin sees every tool");
        assert!(all.is_disjoint(&HIDDEN.iter().copied().collect()));

        let kinds: std::collections::HashMap<&str, Kind> = registry().iter().map(|s| (s.meta().name, s.meta().kind)).collect();
        for role in [Role::Viewer, Role::Auditor] {
            let writes: Vec<_> = tools(role).into_iter().filter(|n| kinds[n] == Kind::Mutation && !PERSONAL.contains(n)).collect();
            assert!(writes.is_empty(), "{role:?} can change {writes:?}");
        }

        let developer = tools(Role::Developer);
        for name in &developer {
            let area = name.split('.').next().unwrap_or_default();
            assert!(!matches!(area, "settings" | "audit"), "a developer has {name}");
            assert!(area != "users" || *name == "users.list", "a developer has {name}");
        }
        assert!(developer.contains("api_keys.list") && developer.contains("system.status"));
    }
}
