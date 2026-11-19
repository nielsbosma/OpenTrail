# One image for the whole app: web app, API, MCP and the job worker, as a single static-ish binary
# on a distroless base (no shell, no package manager), running as a non-root user.
#   docker run -p 3000:3000 -e DATABASE_URL=... -e APP_SECRET=... -e APP_URL=https://trail.example.com \
#     ghcr.io/nielsbosma/opentrail
# ROLE=web or ROLE=worker runs only one half; the default runs both.

FROM node:26-alpine@sha256:c6ad9e9c1d80f6eb607c26b4b0a132ff7a2344fad1c78ea3baa2a7d4c8de6d94 AS web
ENV PNPM_HOME=/pnpm PATH=/pnpm:$PATH COREPACK_ENABLE_DOWNLOAD_PROMPT=0
# Node 25 and later no longer ship corepack.
RUN npm install -g corepack@0.36.0 && corepack enable
WORKDIR /src
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml tsconfig.base.json ./
COPY web/package.json web/
RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm install --frozen-lockfile --filter @opentrail/web
COPY web web
RUN pnpm --filter @opentrail/web build

FROM rust:1.99-slim@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7 AS server
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
