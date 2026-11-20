# One image for the whole app: web app, API, MCP and the job worker, as a single static-ish binary
# on a distroless base (no shell, no package manager), running as a non-root user.
#   docker run -p 3000:3000 -e DATABASE_URL=... -e APP_SECRET=... -e APP_URL=https://trail.example.com \
#     ghcr.io/nielsbosma/opentrail
# ROLE=web or ROLE=worker runs only one half; the default runs both.

FROM node:26-alpine@sha256:6bd9ab43c0ec0190ca7b2d193906cb8b7edf6e7f4afa43ddeccd6452710bffb5 AS web
ENV PNPM_HOME=/pnpm PATH=/pnpm:$PATH COREPACK_ENABLE_DOWNLOAD_PROMPT=0
# Node 25 and later no longer ship corepack.
RUN npm install -g corepack@0.36.0 && corepack enable
WORKDIR /src
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml tsconfig.base.json ./
COPY web/package.json web/
COPY sdk/package.json sdk/
RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm install --frozen-lockfile --filter @opentrail/web --filter @opentrail/browser
COPY sdk sdk
COPY web web
RUN pnpm --filter @opentrail/web build

FROM rust:1.99-slim@sha256:b8e062b1c3a80183df83d9ba187cadd2b5ab84bc78d453dbfeb2fccd5f982247 AS server
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# Compiled into the binary. Set by CI from the release tag; empty means a local build.
ARG OPENTRAIL_VERSION=
ENV OPENTRAIL_VERSION=$OPENTRAIL_VERSION
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin opentrail && cp target/release/opentrail /opentrail

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:08476c244888bfb425bae1ae4dd7c17f093e972aab8426883d97f3c6f280f7c1
COPY --from=server /opentrail /app/opentrail
COPY --from=web /src/web/dist /app/web
ENV PORT=3000 WEB_DIST=/app/web APP_ENV=production
USER nonroot
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s CMD ["/app/opentrail", "health"]
ENTRYPOINT ["/app/opentrail"]
