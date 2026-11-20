-- Projects, their public ingest keys, sessions, the encrypted rrweb stream and what is extracted from it.

create table projects (
  id uuid primary key,
  name text not null check (length(name) between 1 and 100),
  -- Exact origins (scheme://host[:port]) a browser may send recordings from. Empty refuses every origin.
  allowed_origins text[] not null default '{}',
  retention_days integer not null default 90 check (retention_days between 1 and 3650),
  -- Request and response bodies, at most 10 kB each, masked in the browser and again on the server.
  capture_bodies boolean not null default false,
  capture_canvas boolean not null default false,
  -- New sessions a month before ingest answers 429.
  monthly_session_quota integer not null default 100000 check (monthly_session_quota > 0),
  -- The project's data key, wrapped with the recording key-encryption key and bound to the project id.
  data_key text not null,
  archived_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

-- Public by design: the key is in the page source. It can only write recordings to its own project.
create table ingest_keys (
  id uuid primary key,
  project_id uuid not null references projects (id) on delete cascade,
  key text not null unique,
  name text not null,
  last_used_at timestamptz,
  revoked_at timestamptz,
  created_at timestamptz not null default now()
);
create index ingest_keys_project_idx on ingest_keys (project_id);

create table recorded_sessions (
  -- Created in the browser (uuid v7), so retries and several tabs agree on it.
  id uuid primary key,
  project_id uuid not null references projects (id) on delete cascade,
  started_at timestamptz not null,
  last_event_at timestamptz not null,
  first_url text,
  release text,
  -- An id the site chose with identify(). Never an email or name taken automatically.
  user_ref text,
  browser text,
  os text,
  screen_width integer,
  screen_height integer,
  lang text,
  -- /24 for IPv4, /48 for IPv6. The full address is only used for rate limits, in memory.
  ip_prefix text,
  -- Something was dropped: over 4 hours or 100 MB.
  truncated boolean not null default false,
  windows integer not null default 1,
  chunks integer not null default 0,
  bytes bigint not null default 0,
  event_count bigint not null default 0,
  page_count integer not null default 0,
  error_count integer not null default 0,
  failed_request_count integer not null default 0,
  created_at timestamptz not null default now()
);
create index recorded_sessions_project_started_idx on recorded_sessions (project_id, started_at desc);
create index recorded_sessions_project_user_idx on recorded_sessions (project_id, user_ref) where user_ref is not null;
create index recorded_sessions_started_idx on recorded_sessions (started_at);

-- One row per batch a tab sent. `data` is the batch's sanitized events as gzip JSON, sealed with
-- AES-256-GCM under the project's data key and bound to (session, window, seq).
create table replay_chunks (
  session_id uuid not null references recorded_sessions (id) on delete cascade,
  window_id uuid not null,
  seq integer not null check (seq >= 0),
  first_ts timestamptz not null,
  last_ts timestamptz not null,
  event_count integer not null,
  data bytea not null,
  created_at timestamptz not null default now(),
  primary key (session_id, window_id, seq)
);
-- Already compressed and encrypted, so Postgres should not try to compress it again.
alter table replay_chunks alter column data set storage external;

-- How a node the user touched describes itself (role and name as in the accessibility tree).
create table elements (
  session_id uuid not null references recorded_sessions (id) on delete cascade,
  window_id uuid not null,
  node_id integer not null,
  role text,
  name text,
  testid text,
  component text,
  primary key (session_id, window_id, node_id)
);

create table pageviews (
  id bigint generated always as identity primary key,
  session_id uuid not null references recorded_sessions (id) on delete cascade,
  project_id uuid not null,
  window_id uuid not null,
  ts timestamptz not null,
  url text not null,
  title text
);
create index pageviews_session_idx on pageviews (session_id, ts);
create index pageviews_project_ts_idx on pageviews (project_id, ts);

create table interactions (
  id bigint generated always as identity primary key,
  session_id uuid not null references recorded_sessions (id) on delete cascade,
  project_id uuid not null,
  window_id uuid not null,
  ts timestamptz not null,
  url text,
  kind text not null check (kind in ('click', 'dblclick', 'contextmenu', 'input', 'scroll')),
  node_id integer,
  role text,
  name text,
  testid text,
  component text,
  x integer,
  y integer,
  -- How much was typed. What was typed is never stored.
  input_length integer
);
create index interactions_session_idx on interactions (session_id, ts);
create index interactions_project_ts_idx on interactions (project_id, ts);

create table console_logs (
  id bigint generated always as identity primary key,
  session_id uuid not null references recorded_sessions (id) on delete cascade,
  project_id uuid not null,
  window_id uuid not null,
  ts timestamptz not null,
  level text not null check (level in ('log', 'info', 'warn', 'error', 'debug')),
  message text not null,
  stack text
);
create index console_logs_session_idx on console_logs (session_id, ts);
create index console_logs_project_ts_idx on console_logs (project_id, ts);

create table network_requests (
  id bigint generated always as identity primary key,
  session_id uuid not null references recorded_sessions (id) on delete cascade,
  project_id uuid not null,
  window_id uuid not null,
  ts timestamptz not null,
  initiator text not null check (initiator in ('fetch', 'xhr', 'resource')),
  method text not null,
  url text not null,
  -- 0 when the request failed without a response.
  status integer not null,
  duration_ms integer,
  bytes bigint,
  -- Sealed like replay chunks, only for projects with capture_bodies.
  request_body bytea,
  response_body bytea
);
create index network_requests_session_idx on network_requests (session_id, ts);
create index network_requests_project_ts_idx on network_requests (project_id, ts);

create table errors (
  id bigint generated always as identity primary key,
  session_id uuid not null references recorded_sessions (id) on delete cascade,
  project_id uuid not null,
  window_id uuid not null,
  ts timestamptz not null,
  kind text not null check (kind in ('error', 'unhandledrejection')),
  message text not null,
  stack text,
  url text
);
create index errors_session_idx on errors (session_id, ts);
create index errors_project_ts_idx on errors (project_id, ts);

create table vitals (
  id bigint generated always as identity primary key,
  session_id uuid not null references recorded_sessions (id) on delete cascade,
  project_id uuid not null,
  window_id uuid not null,
  ts timestamptz not null,
  url text,
  name text not null check (name in ('LCP', 'INP', 'CLS', 'FCP', 'TTFB')),
  value double precision not null
);
create index vitals_session_idx on vitals (session_id, ts);

create table custom_events (
  id bigint generated always as identity primary key,
  session_id uuid not null references recorded_sessions (id) on delete cascade,
  project_id uuid not null,
  window_id uuid not null,
  ts timestamptz not null,
  name text not null,
  props jsonb
);
create index custom_events_session_idx on custom_events (session_id, ts);
create index custom_events_project_ts_idx on custom_events (project_id, ts);
