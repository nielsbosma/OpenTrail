# Working on OpenTrail

Instructions for coding agents and people. Read the README and SECURITY.md first: recordings hold what people typed and saw, so a change that weakens a control in SECURITY.md is a bug, however convenient.

## Scope

OpenTrail records sessions, shows them, and serves them to agents through REST, MCP and read-only SQL. It never interprets them: no friction signals, summaries, reviews, findings, approvals, issues or follow-ups, and no model calls of its own. A feature that decides what a session means belongs in the agent, not here.

## Golden rules

- **All logic lives in a service** in `crates/core/src/services`: a struct implementing `Service` with `Input`, `Output` (both `JsonSchema`) and a `META`. Register it in `services::registry()`. The RPC endpoint and MCP pick it up from there; REST routes are a table in `crates/server/src/rest.rs`. Handlers never touch the database themselves. The one exception is ingest (`crates/core/src/recording/ingest.rs`): it has no actor, takes binary gzip, and is called only by `crates/server/src/ingest.rs`.
- **Every service declares `Access`**: a `Permission`, `Authenticated` or `Public`. Add permissions in `permissions.rs` and to the roles that need them. The UI only hides things; the service decides.
- **Every change calls `audit(ctx, conn, Entry::new(action, entity_type, id)...)`.** Actions are `<area>.<verb_snake>`. Pass `secret_keys` for anything secret.
- **Descriptions are written for a model**: they become MCP tool descriptions. Full sentences that say what it does and when to use it.
- Mutations run in a transaction. `without_transaction()` only when a failure must still persist writes (attempt counters) or the service calls out to the network. `not_a_tool()` for anything with credentials or the caller's own session; public services are never tools.
- After changing a service's input or output, run `pnpm api:types` and commit `web/src/lib/api-types.gen.ts`.

## Security habits

- Never log or return secrets, tokens or recorded content. Errors shown to users come from `AppError` messages; causes stay in the log.
- Encrypt new secrets with `ctx.keys().encrypt_str(KeyPurpose::..., context, ...)` and a context that names the row, so ciphertext cannot be moved between rows. Recordings use per-project data keys wrapped with `KeyPurpose::RecordingKek`, bound to `project:{id}`; chunks and bodies are sealed with AAD naming their session (see `recording/mod.rs`).
- Every recorded string passes `recording::mask::sanitize` on the server, whatever the SDK did. A new extracted field goes through it too.
- Outbound HTTP to anything configurable goes through `ctx.app.http.public` (blocks private addresses, no redirects). `internal` is only for development mail and a local identity provider.
- Compare secrets with `constant_time_eq`. Store tokens as a MAC or hash, never in the clear.
- Anything the public can reach is rate-limited and bounded (attempts, expiry, size).
- New dependencies: prefer pure Rust, rustls only (no OpenSSL; `cargo deny` refuses it).

## Settings and configuration

- Environment variables are only `DATABASE_URL`, `APP_SECRET`, `APP_URL`, `PORT`, `ROLE`, `APP_ENV`, and the optional `SETUP_CODE`. Runtime configuration belongs in Admin settings.
- A new setting: a field on its group in `crates/core/src/settings.rs` (list secrets in `SECRETS`), validation in `admin::validate`, and a line in the `GROUPS` table in `web/src/pages/settings/AdminSettingsPage.tsx`. It is stored encrypted if secret, cached, audited, and can be pinned with `OPENTRAIL_<GROUP>_<FIELD>` (an empty value pins nothing).

## Data

- Postgres 18 only, and Postgres is the only store. Do not add a bucket, queue or cache service.
- Migrations are numbered SQL files in `crates/db/migrations`. Add a new one; never edit one that has been released. Use `check` constraints for enums and `timestamptz` for times. Queries are written by hand with `sqlx::query` and bound parameters; never format input into SQL.
- The audit log is append-only by trigger. Do not work around it.
- Per-project SQL logins need a database user with `CREATEROLE`. `opentrail_db::role_support` probes it; code that creates roles must handle the answer being no.

## UI

- React, Tailwind and shadcn/ui components in `web/src/components/ui`. Data: `useApi(name, input)` and `useApiMutation(name, { invalidates })`; errors toast unless `silent`.
- The CSP allows no inline script and no external origins: no CDNs, no inline `<script>`, no `eval`.
- **Labels use AP-style Title Case** (headings, buttons, tabs, field labels, menu items): "Create API Key", "Set Up Two-Factor Authentication". Descriptions, hints, placeholders and toasts are sentence case.
- Neutral greys, one indigo accent, subtle borders, light and dark.
- The UI is English only. Still write texts with `t('Text {name}', { name })` from `@/lib/i18n`, `msg()` where a text is defined in a table, and `tx()` for links or code inside a sentence.

## Checks before you finish

CI runs only when an image is built (`main` and releases), so pull requests have no checks and these are the gate. `pnpm stack:up` builds and runs the image locally at http://localhost:8091 without GitHub; `pnpm stack:fresh` does it from empty and deletes the local data.

```sh
pnpm infra:up
pnpm lint && pnpm typecheck
pnpm test:e2e     # the test site recorded end to end, in Docker
TEST_DATABASE_URL=postgres://opentrail:opentrail@localhost:5490/opentrail_test pnpm test
cargo fmt --all --check
```

## Style

- Comments only where they say why, never what. Keep them rare.
- Follow the nearest existing example; consistency beats cleverness.
- Never commit secrets. `.env` and `.env.local` are gitignored.
- Text files use LF line endings (`.gitattributes`).
