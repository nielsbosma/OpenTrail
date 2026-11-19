<div align="center">

<img src="web/public/favicon.svg" width="72" height="72" alt="OpenTrail logo">

# OpenTrail

**Session recording you run yourself, made for AI agents.**<br>
Record what people do on your site (the page, mouse, clicks, console and network), replay it, and let an agent slice out what it needs through an API or MCP.

[![CI](https://github.com/nielsbosma/OpenTrail/actions/workflows/ci.yml/badge.svg)](https://github.com/nielsbosma/OpenTrail/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-4f46e5.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-axum%20%2B%20sqlx-b7410e?logo=rust)](crates)
[![Postgres 18](https://img.shields.io/badge/Postgres-18-336791?logo=postgresql&logoColor=white)](#deploy)

</div>

## Status

OpenTrail is being built in phases. **Phase 1 is done**: the service itself, with accounts, single sign-on, roles, API keys, OAuth for MCP clients, a hash-chained audit log, the admin settings, the image and the deploy. Recording (an rrweb snippet and an ingest endpoint), the replay viewer and the query API come in the next phases. Until then there is nothing to record with.

## What it is, and what it is not

OpenTrail does three things:

1. **Records** web sessions with [rrweb](https://github.com/rrweb-io/rrweb): the DOM and its changes, mouse movement, clicks, scrolling, console output and network requests.
2. **Shows** them: a list of sessions and a player with the console and network beside it.
3. **Serves** them to agents: REST and MCP calls that filter sessions and cut out events, console lines and requests by time, page, user or kind, plus a read-only SQL login per project.

It does not interpret anything. There are no friction signals, summaries, reviews, findings, approvals, issues or follow-ups, and OpenTrail never calls a model. That is the agent's job; OpenTrail gives it the data.

Everything is stored in Postgres. There is no bucket, queue or second database to run.

## Deploy

OpenTrail is one container image plus Postgres 18.

### On Sliplane

[![Deploy to Sliplane](https://sliplane.opendeploy.cloud/button.svg)](https://sliplane.opendeploy.cloud/?repo=https://github.com/nielsbosma/OpenTrail)

The button reads [sliplane.yaml](sliplane.yaml). It sets up a managed Postgres 18 and OpenTrail with a fresh `APP_SECRET`, optionally with Resend for email, and its finish page opens setup with the setup code filled in. From a terminal, `SLIPLANE_TOKEN=api_rw_org_... ./deploy/sliplane.sh` does the same over the Sliplane API.

### With Docker

```sh
docker run -d --name opentrail -p 3000:3000 \
  -e DATABASE_URL=postgres://opentrail:secret@db:5432/opentrail \
  -e APP_SECRET="$(openssl rand -base64 48)" \
  -e APP_URL=https://trail.example.com \
  ghcr.io/nielsbosma/opentrail:latest
```

| Variable | |
| --- | --- |
| `DATABASE_URL` | Postgres 18 or later |
| `APP_SECRET` | 32+ random characters. Every key derives from it: keep it safe and never change it |
| `APP_URL` | The public https address |
| `SETUP_CODE` | Optional: your own setup code (24+ characters) instead of the one in the log |
| `ROLE` | `all` (default), `web` or `worker` |

The image is distroless, runs as a non-root user, works with a read-only root file system, and is multi-arch (amd64, arm64). Tags: `:latest`, `:1.2.3` and `:1.2` for releases, `:edge` for `main`.

### The database user

Each project will get its own read-only SQL login (`ot_<install>_<project>`), so an agent can query that project's data and nothing else. For that, the user in `DATABASE_URL` needs `CREATEROLE`. Sliplane's `app` user has it. On your own Postgres without it, OpenTrail still runs and SQL access stays off. **Settings → System** and the startup log say which applies.

### First run

The container log prints a **setup code**, so nobody else can claim a fresh install (or set your own with `SETUP_CODE` and open `/setup#code=…`). Enter it and create the first admin. Email through **Resend** or your own **SMTP** server is optional when everyone signs in with single sign-on. Everything else is set in the browser, and any setting can be pinned with an environment variable such as `OPENTRAIL_SECURITY_ENFORCE_TWO_FACTOR=true`. An empty variable pins nothing.

## Sign-in and roles

- Passwords (Argon2id) with TOTP two-factor and recovery codes, passkeys, and single sign-on through any OpenID Connect provider (Microsoft Entra ID, Google, Okta and others).
- Four roles, checked on the server for every call: **Admin**, **Developer** (manages projects and API keys), **Viewer** (watches sessions) and **Auditor** (reads the audit log).
- API keys with scopes for scripts, and OAuth 2.1 for MCP clients such as Claude: add a custom connector with `https://<your-install>/mcp` and approve it.

## Use the API

Create a key under **Settings → API Keys**, then:

```sh
curl -H "Authorization: Bearer otk_..." https://trail.example.com/api/v1/me
```

The OpenAPI document is at `/api/v1/openapi.json`, and the **API Explorer** at `/api-explorer` sends live requests and writes curl, JavaScript or Python. Every call also exists as `POST /api/rpc/<service>` and as an MCP tool, with the same permission checks.

## How it fits together

Rust (axum, sqlx) in `crates/`: `crypto` (keys, encryption, tokens; no I/O), `db` (migrations), `core` (services, sign-in, email, the job worker) and `server` (HTTP, REST, MCP). The web app is React, Vite and Tailwind in `web/`. Every feature is a service, so the web app, REST and MCP all go through the same permission checks and audit trail.

## Security

Secrets are encrypted at rest with keys derived per purpose, tokens are stored only as MACs or hashes, outbound requests to configurable addresses cannot reach private networks, the web app runs under a strict CSP, and every service checks permissions on the server. [SECURITY.md](SECURITY.md) has the threat model and controls. Please report vulnerabilities privately through GitHub's security advisories.

## Contributing

Issues and pull requests are welcome. Run it locally with:

```sh
pnpm install
pnpm infra:up            # Postgres 18 and Mailpit in Docker
cp .env.example .env
pnpm dev                 # web app on http://localhost:5173
```

[CONTRIBUTING.md](CONTRIBUTING.md) has the commands, checks and release process, and [CLAUDE.md](CLAUDE.md) the conventions.

## License

[MIT](LICENSE)
