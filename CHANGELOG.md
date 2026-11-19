# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/) and versions follow [Semantic Versioning](https://semver.org/). Database migrations always run forward.

## [Unreleased]

### Added

- The service itself: first-run setup with a setup code, accounts with passwords, TOTP two-factor and passkeys, single sign-on through OpenID Connect, and the roles Admin, Developer, Viewer and Auditor.
- API keys with scopes, a REST API with OpenAPI and an API Explorer, and an MCP server with OAuth 2.1 for clients such as Claude.
- A hash-chained, append-only audit log that can be verified from Settings.
- Admin settings for the organisation, email (Resend or SMTP), security, updates and single sign-on, each of which can be pinned with an `OPENTRAIL_<GROUP>_<FIELD>` variable.
- Settings → System and the startup log say whether the database user can create roles, which per-project SQL logins will need.
- A distroless image for Postgres 18, `sliplane.yaml` for the Deploy on Sliplane button, and `deploy/sliplane.sh` for the Sliplane API.
