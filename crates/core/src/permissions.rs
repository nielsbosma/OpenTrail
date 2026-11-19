use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

macro_rules! permissions {
    ($($variant:ident = $name:literal : $description:literal,)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
        pub enum Permission {
            $(#[serde(rename = $name)] $variant,)*
        }

        impl Permission {
            pub const ALL: &'static [Permission] = &[$(Permission::$variant,)*];

            pub fn as_str(self) -> &'static str {
                match self { $(Permission::$variant => $name,)* }
            }

            pub fn description(self) -> &'static str {
                match self { $(Permission::$variant => $description,)* }
            }

            pub fn parse(value: &str) -> Option<Permission> {
                match value { $($name => Some(Permission::$variant),)* _ => None }
            }
        }
    };
}

permissions! {
    OrgRead = "org.read": "See organisation details",
    UsersRead = "users.read": "See who has an account",
    UsersManage = "users.manage": "Invite people, change roles and deactivate accounts",
    SettingsManage = "settings.manage": "Change Admin settings: organisation, email, single sign-on, security",
    AuditRead = "audit.read": "Browse and verify the audit log",
    SessionsRead = "sessions.read": "Watch recorded sessions and read their events, console, network and errors",
    SessionsDelete = "sessions.delete": "Delete recorded sessions, and everything recorded for a user",
    ProjectsManage = "projects.manage": "Create projects, their public keys, allowed origins, retention and masking",
    ApiKeysManage = "api_keys.manage": "Create API keys that act with your permissions",
    SystemRead = "system.read": "See version, migrations and background jobs",
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    Developer,
    Viewer,
    Auditor,
}

pub struct RoleInfo {
    pub label: &'static str,
    pub description: &'static str,
    pub permissions: &'static [Permission],
}

use Permission::*;

impl Role {
    pub const ALL: [Role; 4] = [Role::Admin, Role::Developer, Role::Viewer, Role::Auditor];

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Developer => "developer",
            Role::Viewer => "viewer",
            Role::Auditor => "auditor",
        }
    }

    pub fn parse(value: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.as_str() == value)
    }

    pub fn info(self) -> RoleInfo {
        match self {
            Role::Admin => RoleInfo {
                label: "Admin",
                description: "Everything, including users, settings and single sign-on",
                permissions: Permission::ALL,
            },
            Role::Developer => RoleInfo {
                label: "Developer",
                description: "Watches sessions, manages projects and creates API keys for agents",
                permissions: &[OrgRead, UsersRead, SessionsRead, ProjectsManage, ApiKeysManage, SystemRead],
            },
            Role::Viewer => RoleInfo {
                label: "Viewer",
                description: "Watches recorded sessions, changes nothing",
                permissions: &[OrgRead, UsersRead, SessionsRead],
            },
            Role::Auditor => RoleInfo {
                label: "Auditor",
                description: "Reads the audit log and system status, changes nothing",
                permissions: &[OrgRead, UsersRead, AuditRead, SystemRead],
            },
        }
    }

    pub fn permissions(self) -> BTreeSet<Permission> {
        self.info().permissions.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_has_everything_and_names_round_trip() {
        assert_eq!(Role::Admin.permissions().len(), Permission::ALL.len());
        for p in Permission::ALL {
            assert_eq!(Permission::parse(p.as_str()), Some(*p));
            assert_eq!(serde_json::to_string(p).ok(), Some(format!("\"{}\"", p.as_str())));
        }
        for r in Role::ALL {
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
    }

    #[test]
    fn read_only_roles_cannot_write() {
        for role in [Role::Auditor, Role::Viewer] {
            let p = role.permissions();
            assert!(
                !p.contains(&ProjectsManage) && !p.contains(&SessionsDelete) && !p.contains(&SettingsManage) && !p.contains(&UsersManage)
            );
        }
    }
}
