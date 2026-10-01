# CLAUDE.md

Project context for Claude Code sessions. Read this before doing anything non-trivial.

---

## What this is

**LiteRouter** — a lightweight LLM API gateway. One Rust binary + SQLite + Vue 3 SPA. Aggregates multiple upstream LLM providers (OpenAI, Claude, relay stations) behind a single endpoint, issues its own `sk-…` tokens to downstream users, and handles model routing, multi-channel failover, protocol conversion, and usage tracking.

Single repo, two deployable surfaces:

- **Production**: `docker compose up -d --build` (three-stage Dockerfile: frontend → backend → alpine runtime)
- **Dev (host-native)**: `./run.sh start` (orchestrates `cargo run` + `vite` together, **not** separate commands)

---

## Dev deployment — important

**Always use `./run.sh` for dev.** Do NOT start the backend and frontend in separate shells — the proxy in `vite.config.js` and the CORS setup assume they're co-launched by the same script.

```bash
./run.sh start [port]    # port defaults to 3000; frontend dev server is fixed at 5173
./run.sh stop
./run.sh status
./run.sh logs [be|fe|all]
```

- Backend listens on the chosen `port` (default 3000)
- Frontend dev server listens on **5173** with `/api` and `/v1` proxied to the backend — open **http://localhost:5173**, not :3000, in dev
- Logs: `.run/logs/backend.log`, `.run/logs/frontend.log`
- PIDs: `.run/backend.pid`, `.run/frontend.pid`
- DB: `data/literouter.db` (override with `LITEROUTER_DB=/some/path`)

If you only need to bounce the backend (e.g. after a `db.rs` change), `kill $(cat .run/backend.pid)` then `cd backend && env PORT=$PORT LITEROUTER_DB=$DB cargo run &` works, but for a clean reset prefer `./run.sh stop && ./run.sh start`.

---

## Project layout

```
.
├── run.sh                       # dev start/stop script (see above)
├── docker-compose.yml           # prod: single container, exposes PORT
├── Dockerfile                   # 3-stage: node build → cargo build → alpine runtime
├── README.md / README.zh-CN.md
├── backend/
│   ├── Cargo.toml
│   ├── migrations/              # SQL schema migrations, embedded by sqlx::migrate!
│   │   ├── 0001_channels.sql
│   │   ├── …
│   │   └── 0026_ui_language.sql
│   ├── tests/                   # integration tests (see Testing)
│   │   ├── support/mod.rs       # shared fixtures — not a test target itself
│   │   ├── relay_contract.rs    # wiremock as a real upstream
│   │   ├── manage_users.rs / manage_data.rs
│   │   └── convert_tests.rs / stream_convert.rs / small_helpers.rs
│   └── src/
│       ├── lib.rs               # library root: build_state() + build_router()
│       ├── main.rs              # thin shell: env → pool → state → router → serve
│       ├── state.rs             # AppState: pool + http client + sessions + breaker
│       ├── db.rs                # init_pool(), PBKDF2 helpers, log retention
│       ├── auth.rs              # setup/login/logout/me/password + json_err
│       ├── users.rs             # /api/users CRUD (admin only)
│       ├── admin.rs             # channels/tokens/mappings/logs/usage handlers
│       ├── settings.rs          # debug-logging / language / breaker-config endpoints
│       ├── breaker.rs           # circuit-breaker state machine + /api/breaker/*
│       ├── breaker_probe.rs     # background half-open probe ticker
│       ├── proxy.rs             # /v1/chat/completions, /v1/messages — the actual relay
│       └── convert.rs           # OpenAI ⇄ Anthropic protocol conversion
├── frontend/
│   ├── vite.config.js           # proxies /api + /v1 → backend, port 5173
│   ├── vitest.config.js         # jsdom env for the unit suites
│   ├── scripts/check-i18n.mjs   # locale key parity (see Testing)
│   ├── tests/                   # vitest suites
│   └── src/
│       ├── main.js              # bootstrap: locale, then mount
│       ├── App.vue              # el-config-provider — wires Element Plus to the locale
│       ├── router.js            # global beforeEach checks /api/setup-status
│       ├── api.js               # axios instance + fetch wrappers for every endpoint
│       ├── i18n/index.js        # createI18n, setLocale, translate
│       ├── language.js          # shared language switch (sidebar + Settings)
│       ├── debug.js             # shared debug-logging flag
│       ├── breaker.js           # shared breaker snapshot
│       └── views/
│           ├── Setup.vue        # first-run wizard
│           ├── Login.vue
│           ├── Layout.vue       # shell: sidebar, user menu, change-password dialog
│           ├── Channels.vue / Models.vue
│           ├── Tokens.vue / Mappings.vue
│           ├── Logs.vue / LogDetail.vue
│           ├── Usage.vue
│           ├── Users.vue        # admin-only
│           └── Settings.vue     # language (and growing)
└── data/literouter.db           # gitignored, SQLite
```

---

## Tech stack

- **Backend**: Rust, `axum` 0.7, `sqlx` 0.7 (sqlite + `migrate`), `reqwest`, `serde`, `tokio`. Password hashing via `pbkdf2` (HMAC-SHA256, 100k iters) + `sha2` + `rand` — **pure Rust, no C deps**, alpine builds are fine.
- **Frontend**: Vue 3, Vite, Element Plus, `vue-router`.
- **Storage**: SQLite, single file. Schema is **owned by sqlx migration files** — `db.rs::init_pool` calls `sqlx::migrate!("./migrations").run(&pool)` and must NOT contain inline `CREATE TABLE`.

---

## Multi-account model

| Resource | admin | regular user |
|---|---|---|
| Channels | CRUD | hidden (use `/v1/models`) |
| Mappings (model routing) | CRUD | hidden |
| Tokens | CRUD, can assign owner | only their own |
| Logs / usage | all | filtered to their tokens |
| Users | CRUD | self-service password change |

- **First-run wizard**: `GET /api/setup-status` returns `{ needsSetup: true }` when `users` is empty. Router intercepts in `frontend/src/router.js` and redirects to `/setup`. `POST /api/setup` is idempotent (403 once `users` is non-empty).
- **Sessions**: in-memory `HashMap<session_id, SessionInfo{user_id, is_admin}>`. **Process restart = everyone logs out.** This is intentional — the project has no session store.
- **Password hashing**: PBKDF2-HMAC-SHA256, 100k iters, 32-byte output, 16-byte salt. Helpers in `db.rs::hash_password` / `verify_password` (constant-time compare). Never roll your own crypto.
- **Token ownership migration**: on startup, any `tokens` row with `user_id IS NULL` is reassigned to the first admin (`ORDER BY id ASC LIMIT 1`). No-op on fresh DBs and on already-clean DBs.

**At least one admin must always remain.** Both `DELETE /api/users/:id` and the `is_admin → 0` path in `PUT /api/users/:id` reject if it would leave the system with zero admins.

---

## Database migrations

`backend/migrations/` is the single source of truth. 26 files represent the actual schema history (0001 includes `kind` for legacy fidelity; 0003 drops it; 0020–0022 reworked the circuit breaker). sqlx embeds them at compile time, so:

1. **Adding a migration**: create `00NN_short_name.sql`, run `./run.sh stop && ./run.sh start`. The dev DB picks it up automatically.
2. **Editing an existing migration**: **do not**, unless you also bump every downstream file. The migration history is immutable — write a new one.
3. **Resetting for a fresh DB**: `./run.sh stop && rm data/literouter.db && ./run.sh start`. The wizard reappears.

For production, sqlx's `migrate` runs on container start, same way.

---

## API surface (quick reference)

**Public** (no auth):
- `GET  /v1/models` — merged model list across enabled channels
- `POST /v1/chat/completions` — OpenAI protocol, supports `stream: true`
- `POST /v1/messages` — Anthropic protocol, supports streaming
- `GET  /api/setup-status` — `{ needsSetup, authenticated, is_admin }`
- `POST /api/setup` — `{ username, password }` (only when users empty)
- `POST /api/login` — `{ username, password }` → `{ session, is_admin, username }`

**Bearer session** (`Authorization: Bearer <session_id>`):
- `POST /api/logout`
- `GET  /api/me`
- `POST /api/password` — `{ old_password, new_password }`
- `GET/POST /api/tokens`, `PUT/DELETE /api/tokens/:id` — user-scoped
- `GET  /api/logs`, `GET /api/usage` — user-scoped

**Admin only**:
- `GET/POST /api/channels`, `PUT/DELETE /api/channels/:id`
- `POST /api/channels/fetch-models`
- `POST /api/channels/test-model`
- `GET/POST /api/mappings`, `PUT/DELETE /api/mappings/:id`
- `GET/POST /api/users`, `PUT/DELETE /api/users/:id`

---

## Key conventions

- **Auth headers**: `Authorization: Bearer <session_id>` for `/api/*`. `/v1/*` uses `Authorization: Bearer sk-…` (the *token*, not a session) — the gateway resolves the token to a user for ownership/quotas.
- **Sessions vs tokens**: a *session* is an admin-UI login (in-memory, short-lived, lost on restart). A *token* (`sk-…`) is what downstream SDKs use to call `/v1/*` — these are persistent and quota-tracked.
- **Two-protocol channels**: a channel can serve both OpenAI and Anthropic with the same key (e.g. Volcengine Ark). Fill in both URLs; the proxy picks the right one based on client protocol.
- **Wildcard models**: `models = "*"` matches any client model name. Useful during provider migration.
- **Mappings**: client model → ordered list of upstream models. The proxy walks the list and falls back on **any** non-2xx or transport error — a 400/422 still moves on to the next candidate rather than being returned immediately. What differs by status is only whether the failure is fed to the circuit breaker (400/422 are exempt) and the *synthesized* final status when every candidate fails: all-429 → 429 + `Retry-After`, all transport errors → 504, any mix → 502.
- **Client disconnect**: axum's `Body::stream` + a cancellation token aborts the upstream reqwest call when the downstream disconnects. Don't waste upstream quota.
- **Breaker backoff is exponential**, despite what `migrations/0022_breaker_linear_backoff.sql` says in its comment (that comment describes 30 → 60 → 90; the code and its unit tests implement doubling, 30 → 60 → 120 → …, capped at `breaker_max_delay_secs`). The migration file stays as-is — history is immutable — so treat `breaker.rs` as the source of truth and the migration name/comment as historical.

---

## Testing

`backend/tests/` holds 8 integration suites (~236 tests) plus unit tests inside `src/`. They all run in-process:

- `src/lib.rs` exposes `build_state(pool)` and `build_router(state)`. `src/main.rs` is a thin shell that calls them — **all routing lives in the lib** so tests can mount the real `Router` via `tower::ServiceExt::oneshot`. Never move route definitions back into `main.rs`.
- Matching a SQLite error code goes through `db::is_unique_violation`, never a string literal. sqlx reports the **primary** code (`2067`), not the extended one (`20602`); a literal that doesn't match silently degrades a 409 into a 500. This bit two handlers before it was centralized.
- `tests/support/mod.rs` is the shared fixture module (not a test target itself): `TestDb` gives each test its own SQLite file in a tempdir, `Harness` bundles db + router + breaker, `call`/`call_json`/`login` wrap the oneshot plumbing.
  - **One SQLite file per test, deliberately.** A shared in-memory pool flakes with `database is locked` because SQLite serializes writers per-database and `cargo test` runs suites in parallel threads.
  - `shared_hash()` computes the PBKDF2 hash **once per test binary** via `OnceLock`. Production `PBKDF2_ITERATIONS` stays at 100k — do not add a test-only env override to weaken the work factor.
- Relay contract tests use `wiremock` as a real upstream server; the rest drive the router directly.
- `proxy.rs` handlers use `OptionalConnectInfo`, which yields `None` when axum didn't install the extension. That keeps `oneshot` working in tests without changing production behavior (the extension is always present there).

Run everything:

```bash
cd backend && cargo test --all-targets   # unit + integration
cd frontend && npm test                  # locale parity + vitest suites (see below)
```

**Frontend tests** (`frontend/tests/*.spec.js`, vitest + @vue/test-utils + jsdom, 8 suites / 110 tests):

- `npm test` = `check:i18n` then `test:unit`. Both are blocking in CI.
- `scripts/check-i18n.mjs` is zero-dependency and fails when en-US and zh-CN key sets drift, when a zh-CN value is empty, or when both locales carry the identical string. vue-i18n falls back silently on a missing key, so nothing else would catch it.
- `src/views/*` and `src/router.js` are loaded with **lazy `import()`**, so a suite that mounts them must `await import(...)` at top level rather than a static `import`. See `tests/layout.spec.js`.
- `src/router.js` exports a singleton, and vue-router **skips the guard entirely** on a `push` to the path it is already on. Reusing the instance across cases makes any test that revisits the previous test's landing path pass vacuously. `tests/router_guard.spec.js` calls `vi.resetModules()` and re-imports per test.
- `src/debug.js` and `src/breaker.js` are module-level singletons on purpose (one flag shared across views). Same `vi.resetModules()` treatment; their `vi.mock('../src/api')` factories need `vi.hoisted` for the spies, or hoisting runs before the bindings exist.
- Element Plus `@closed` fires from the leave transition, which jsdom never runs. Drive such handlers through the setup function directly and say so in a comment, rather than reshaping the component to suit the test.
- Element Plus components take **no arbitrary props**: a typo like `:loading` on `el-radio-group` silently falls through to the root element as a plain HTML attribute. If a prop "does nothing", check it exists.
- Language switching has two entry points (sidebar dropdown, Settings page), both routed through `src/language.js`. That module is deliberately **not** in `src/i18n/index.js` — `api.js` imports `translate` from there, so adding an API call would close an import cycle.

CI (`.github/workflows/ci.yml`) runs `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` as **blocking**; the tree is clean as of 715035b, so keep it that way rather than adding `#[allow]`s.

---

## Common tasks

**Add a new admin endpoint:**
1. Handler in `backend/src/admin.rs` (or `users.rs` for user CRUD)
2. Wire it in `main.rs::router()`
3. Mirror it in `frontend/src/api.js`
4. If it's admin-only, guard with `require_admin(...)`. If user-scoped, apply the same `WHERE` filter pattern as tokens/logs/usage.

**Add a new SQL column:**
1. `touch backend/migrations/00NN_*.sql` with the `ALTER TABLE`
2. Bump the corresponding query in `admin.rs` / `proxy.rs` / `db.rs` to read/write it
3. `./run.sh stop && ./run.sh start` — migration runs on the dev DB

**Debug a relay issue:** check `.run/logs/backend.log` for the structured request log line (`channel=… model=… status=… took=…`). For SSE issues, `curl -N` against `/v1/chat/completions` shows the stream raw.

**Reset everything:**
```bash
./run.sh stop && rm -f data/literouter.db && ./run.sh start
# then visit http://localhost:5173 — wizard reappears
```

---

## Things to watch out for

- **Don't put schema in `db.rs`.** It's all in `backend/migrations/*.sql`. The `init_pool` function only opens the pool and runs `migrate!`. If you find yourself writing `CREATE TABLE` inside `db.rs`, stop and write a migration.
- **Don't add `ADMIN_PASSWORD` back.** It was removed; the wizard is the only way to bootstrap.
- **Don't add `reqwest::Client` per request** in `proxy.rs`. There's one shared client in `AppState` for connection pooling — instantiating per request leaks DNS resolvers and burns sockets.
- **Don't expose `api_key` from `/api/channels` to non-admins.** Note that `row_channel` in `admin.rs` returns `api_key` **in full** — the protection is the endpoint's `require_admin(...)` gate, not redaction. That's deliberate: the edit form prefills the field, so redacting would make it impossible to save a channel without retyping the key. If you add a new code path that returns channel rows, put it behind `require_admin` too.
- **Frontend dev URL is `localhost:5173`, not `:3000`.** The proxy is the entire point.
- **Vite 8 (Rolldown-based), so the build needs Node `^20.19` or `>=22.12`.** `build.rollupOptions` is now `build.rolldownOptions` in any warning text you see. One transitive dep — `nopt`, via `js-beautify` ← `@vue/test-utils` — declares Node `^22.22.2`; npm warns `EBADENGINE` on Node 20 but nothing exercises it, and CI pins Node 22.
- **Don't use `:` or `;` in usernames** — they go into route paths and SQL params; the setup endpoint validates.
