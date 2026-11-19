# Security

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub's **Security → Report a vulnerability** on this repository, not in a public issue. Include what an attacker can do and how to reproduce it. You will get an answer within a few days, and credit in the release notes if you want it.

## What OpenTrail protects against

Session recordings hold what people saw and did on a site, so the design assumes:

- **The internet**: password guessing, stolen session cookies, cross-site requests, SSRF through anything that fetches a URL, and clients that send more than they should.
- **A partial compromise**: someone who reads the database but not `APP_SECRET` must not be able to read secrets or forge sessions and tokens. Write access to the database is otherwise out of scope, with one exception: the audit trail cannot be rewritten without it showing.
- **Insiders without the right role**: every operation is checked on the server against the caller's role, whichever way it arrives (web app, REST, MCP).

It does not protect against someone who holds `APP_SECRET` together with the database, or controls the host.

## Controls

**Keys and encryption**
- Every key derives from `APP_SECRET` with HKDF, one key per purpose (settings, recording keys, tokens, audit chain, setup code), so one purpose's key never decrypts another's data.
- Secret settings (API keys, the SMTP password, the single sign-on client secret, the deploy hook) are encrypted in Postgres with context-bound AES-256-GCM.

**Accounts and sessions**
- Passwords are Unicode-normalized and hashed with Argon2id (64 MiB, 3 passes), with a minimum length. Failed sign-ins lock the sign-in for that email and address for a while, and failures from several addresses lock the account; a passkey still signs in. Every attempt is audited.
- TOTP two-factor authentication with single-use recovery codes, which an admin can require for everyone.
- Single sign-on (OpenID Connect) uses the authorization code flow with PKCE (S256), state, nonce and full ID token checks (RS256 signature, issuer, audience, expiry), and needs a `__Host-` cookie from the browser that started it, so a link started by someone else does not sign the visitor in. Discovery and token requests go through the public HTTP client. An account is identified by `iss` and `sub`, and the email must be verified by the provider. Accounts are created with a configured role that is never admin, roles never come from the provider, and admin accounts are never linked automatically. Linking an existing account removes its password, two-factor, passkeys, API keys and OAuth grants, with an audit entry. Single sign-on accounts have no password, passkeys, API keys or MCP access and cannot change their email or use forgot password, and any session from the provider gets no recent confirmation. The two-factor requirement needs `amr` to name a second factor (`mfa`, `otp`, `hwk`). **Admins Only** keeps password sign-in for admins alone. A block at the provider takes effect when the session expires. The settings can only be changed by an admin signed in, never through MCP.
- Passkeys (WebAuthn, verified in-house in Rust): user verification required, origin and RP ID checked, challenges single-use and short-lived, signature counters must increase (authenticators without a counter report 0 throughout), ES256, EdDSA and RSA of 2048 bits or more. A passkey satisfies the two-factor requirement.
- Session cookies are `HttpOnly`, `SameSite=Lax`, `__Host-` prefixed and `Secure` on https, stored only as a hash in the database, with absolute and idle expiry. Cookie-authenticated writes must come from our own origin (`Origin` or `Sec-Fetch-Site`). Requests from another site are refused on `/api` even without a session, and `/api/rpc` requires a JSON content type.
- The first admin can only be created with a one-time setup code: from the server log, or `SETUP_CODE` (24+ characters) when a deployer generates one and passes it in a link's URL fragment. It stops working once the first admin exists, and single sign-on is refused until then.
- API keys and OAuth tokens are stored as hashes. MCP uses OAuth 2.1 with PKCE (S256 only), exact redirect URIs, audience-bound access tokens, and rotating refresh tokens with reuse detection that revokes the whole grant. Creating an API key or approving an MCP client asks for the password or a passkey unless the session confirmed one in the last ten minutes. Password and two-factor resets revoke API keys. Unapproved self-registered clients expire after a week, and registration can be turned off.

**Roles**
- Admin, Developer, Viewer and Auditor, enforced in the service layer. The UI only hides what the server would refuse anyway. MCP clients act with the user's own permissions, and public services are never tools.

**Database access for agents**
- Per-project SQL logins will be separate Postgres roles with `NOINHERIT`, a connection limit, `default_transaction_read_only` and a statement timeout, granted only their project's views. Creating them needs `CREATEROLE`; without it, SQL access stays off. The probe that checks this creates a throwaway role inside a transaction and rolls it back.

**Audit trail**
- Every change is in an append-only log (database triggers refuse updates, deletes and truncation), hash-chained with a key derived from `APP_SECRET`. `audit.verify` checks the chain from the first entry to the head.

**The web and the network**
- Strict CSP (`script-src 'self'`, no inline script, `frame-ancestors 'none'`), HSTS on https, `nosniff`, `no-referrer`, COOP and CORP, and a tight permissions policy.
- Rate limits per address (IPv6 per /64) on sign-in and codes, on top of the database lockouts, with body limits checked after authorization and a cap on requests in flight. The client address comes from `X-Forwarded-For` only as the **Trusted Proxies** setting allows.
- Outbound requests to anything configurable (identity providers, the deploy hook) resolve DNS first and refuse private, loopback, link-local and metadata addresses, and follow no redirects. Proxy environment variables are ignored. Only `APP_ENV=development` lets a single sign-on issuer be on a private address; never set it on a real install.
- SMTP servers get the same address check, resolved once and connected to by that address. The connection must be TLS (implicit or STARTTLS, never a plain-text fallback) with a certificate valid for the configured host before any password is sent.
- The update check sends GitHub nothing but the version in the User-Agent, at most every six hours, and can be turned off. The deploy hook must be a public HTTPS address, only an admin can call it, and every call is audited.

**Supply chain and runtime**
- Rust with `unsafe_code = "forbid"`, rustls only (no OpenSSL), `cargo deny` for advisories and licenses, `pnpm audit` for the web app, and a pinned lockfile for both.
- The image is distroless with no shell, runs as a non-root user, and works with a read-only root file system. Release images carry provenance and an SBOM.

## Running it safely

- Generate `APP_SECRET` with `openssl rand -base64 48` (production refuses example and predictable values), store it in your platform's secret store, and back it up apart from the database. Losing it makes every secret and recording unreadable.
- Serve OpenTrail over https only and set `APP_URL` to that address.
- If your proxy is not on a private network, or clients can reach OpenTrail directly from one, set **Trusted Proxies** in Admin settings → Security.
- Turn on **Require two-factor authentication** in Admin settings → Security.
- With single sign-on, leave **Require a Second Factor from the Provider** on where the provider sends `amr`, otherwise require a second factor at the provider. Keep at least one admin with a password, and consider **Admins Only**.
- Back up Postgres. It holds everything.
