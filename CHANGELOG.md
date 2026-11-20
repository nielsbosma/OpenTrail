# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/) and versions follow [Semantic Versioning](https://semver.org/). Database migrations always run forward.

## [Unreleased]

### Added

- Recording: projects with allowed origins, retention, a monthly session quota and public ingest keys. The browser SDK (`/sdk.js` and `@opentrail/browser`) records with rrweb 2.2.0, plus console, fetch and XHR, errors, Web Vitals, SPA navigation, element descriptions, `identify` and `track`.
- `POST /i/v1/batch`: gzip or plain JSON batches, bounded in size, events and time. The server masks identity and card numbers, removes event handlers and script URLs, masks credentials in URLs, and encrypts each batch under the project's own key.
- The Sessions page and a player with its events, console, network, errors and vitals beside it, and a Projects page with the snippet, keys and capture settings.
- `sessions.list`, `sessions.get`, `sessions.timeline`, `sessions.replay`, `sessions.request_body`, `events.search` and `events.group`, as RPC, REST and (except the raw replay) MCP tools.
- Retention per project in the sweep, `sessions.delete` and `sessions.delete_user`.
- `testsite/`: a site with every recording scenario, and `pnpm test:e2e`, which drives it with Playwright against a fresh image.
- The service itself: first-run setup with a setup code, accounts with passwords, TOTP two-factor and passkeys, single sign-on through OpenID Connect, and the roles Admin, Developer, Viewer and Auditor.
- API keys with scopes, a REST API with OpenAPI and an API Explorer, and an MCP server with OAuth 2.1 for clients such as Claude.
- A hash-chained, append-only audit log that can be verified from Settings.
- Admin settings for the organisation, email (Resend or SMTP), security, updates and single sign-on, each of which can be pinned with an `OPENTRAIL_<GROUP>_<FIELD>` variable.
- Settings → System and the startup log say whether the database user can create roles, which per-project SQL logins will need.
- A distroless image for Postgres 18, `sliplane.yaml` for the Deploy on Sliplane button, and `deploy/sliplane.sh` for the Sliplane API.
