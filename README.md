English | [简体中文](README.zh-CN.md)

<p align="left">
  <img src="assets/logo.svg" alt="LiteRouter" width="360">
</p>

# LiteRouter

A lightweight LLM API gateway. Aggregates multiple upstream LLM services (OpenAI / Claude / relay stations, etc.) behind a single unified endpoint, issues its own access keys to downstream clients, and provides model routing, multi-channel failover, and usage tracking. Single Rust binary + SQLite, no external dependencies, ready out of the box.

## Features

- **Unified access**: one endpoint at `http://your-host:3000/v1`, compatible with both protocols:
  - `POST /v1/chat/completions` — OpenAI protocol (`Authorization: Bearer sk-...`)
  - `POST /v1/messages` — Anthropic Messages protocol (`x-api-key: sk-...`)
  - Both support streaming (SSE) passthrough
- **Channel management**: upstream channel keys (OpenAI / Claude / relay stations) are stored only in the gateway and never exposed; supports dual-protocol URLs (for channels serving both protocols with the same key, such as Volcengine Ark, just fill in both URLs)
- **Channel routing**: routes requests to upstream channels that support the requested `model`
- **Protocol conversion**: automatically converts bidirectionally when the client protocol doesn't match the channel's (Anthropic Messages ⇄ OpenAI ChatCompletions), including streaming and tool calls. For example, an OpenAI-protocol channel can directly serve Claude Code (an Anthropic-protocol client) with zero configuration
- **Model routing (Mappings)**: maps the model name in client requests (e.g. `my-model`) to a list of real upstream model IDs, one-to-many with adjustable order; requests are forwarded in list order and automatically fall back to the next target when earlier ones fail or have no available channel
- **Multi-channel failover**: on upstream transport errors or retryable status codes (408 / 429 / 5xx / 524), automatically switches to the next candidate channel; non-retryable 4xx errors are returned to the client as-is
- **Internal tokens + quotas**: issue `sk-` keys for your own services/users, with per-token RPM limits and daily token-usage limits (429 on exceeding)
- **Usage tracking & call logs**: records the channel, model, status code, and prompt / completion / total tokens of every request, with a usage dashboard in the admin UI
- **Client-disconnect cancellation**: upstream requests are cancelled automatically when the client disconnects, saving upstream quota
- **Zero maintenance**: SQLite storage, no external dependencies, database schema migrates automatically

## Tech Stack

- Backend: Rust (axum + sqlx/SQLite + reqwest)
- Frontend: Vue 3 + Vite + Element Plus

## Deployment

### Deploy from GHCR (recommended)

Prebuilt images are published to GitHub Container Registry. The list of published tags lives at <https://github.com/PandaJiuJiu/LiteRouter/pkgs/container/literouter>; pin a tag in production, or use `latest` if you'd rather track the most recent release:

```bash
docker pull ghcr.io/qihangkong/literouter:latest
```

**The package is private**, so log in first with a GitHub token that has the `read:packages` scope (a classic PAT with that scope, or a fine-grained PAT granting read access to this package):

```bash
echo "$GHCR_TOKEN" | docker login ghcr.io -u <your-github-username> --password-stdin
```

Then run it. Save this as `compose.yml` next to your data directory:

```yaml
services:
  literouter:
    image: ghcr.io/PandaJiuJiu/literouter:latest
    container_name: literouter
    ports:
      - "${PORT:-3000}:3000"
    environment:
      # Required — see the warning below
      - LITEROUTER_DB=/app/data/literouter.db
    volumes:
      - ./data:/app/data
    restart: unless-stopped
```

```bash
mkdir -p data && docker compose up -d
```

Visit `http://your-host:3000` — the system will walk you through creating the first admin account.

> **Warning: `LITEROUTER_DB` must be set explicitly when running the prebuilt image.**
> Without it the backend defaults to `literouter.db`, which resolves to `/app/literouter.db`
> — *outside* the mounted volume. The database would then live in the container's writable
> layer and be silently destroyed the next time the container is recreated, with no error.
> The `docker-compose.yml` in this repository sets it for you; your own compose file must too.

Notes:

- The image is a three-stage build (frontend → backend → runtime), final stage based on `alpine:3.20`, roughly 22 MB, no external dependencies
- **linux/amd64 only.** On Apple Silicon / arm64, either enable emulation
  (`podman run --arch amd64` / Docker Desktop's default) or [build from source](#building-from-source)
- SQLite data is persisted in the `./data` directory — back it up before upgrading, see below

### Upgrading

```bash
docker compose pull && docker compose up -d
```

Or, to pin a specific version, change the `image:` tag first.

What to expect:

- **Schema migrations run automatically** on startup. Since v0.0.2 the migrations are additive (`INSERT OR IGNORE` into `settings`), so upgrading is a no-op for existing data
- **Migrations are not reversible** — there is no supported downgrade path. Copy `./data` before upgrading
- **Everyone must log in again.** Sessions are held in memory only and do not survive a restart. This is intentional; the project has no session store
- **Downstream API tokens (`sk-…`) are unaffected** — clients calling `/v1/*` keep working without being reissued

### Building from source

```bash
git clone https://github.com/PandaJiuJiu/LiteRouter.git
cd LiteRouter
docker compose up -d --build
```

Notes:

- `docker-compose.yml` depends on the `Dockerfile` in the same directory; both work without modification
- On older Docker versions, use `docker-compose` (with hyphen) instead of `docker compose`
- If Docker Hub is unreachable, switch the registry mirror via a build arg:

```bash
docker compose build --build-arg REGISTRY=docker.io/library
```

### Account model

- First visit redirects to an **initialization wizard** that creates the first admin account (password ≥ 8 chars)
- Admins manage users from the **Users** page: create, reset password, promote/demote, delete
- **Channels** and **mappings** are shared infrastructure managed by admins; visible to all logged-in accounts but only writable by admins
- **Tokens** are per-user. Each user creates and manages their own; admins see all and can create tokens on behalf of others
- **Logs** and **usage** are auto-scoped per user; admins see everything
- At least one admin must always remain to prevent lockout

### Changing the port

The host port is determined by the `ports` mapping in your compose file, overridable via the `PORT` environment variable (the container always listens on 3000):

```bash
PORT=8080 docker compose up -d      # access at http://your-host:8080
```

Note that `PORT` only affects the **host-side** port mapping; the in-container process port needs no change. To revert to the default, drop the `PORT=` prefix and run `up -d` again.

### Environment variables

| Variable | Default | Description |
|---|---|---|
| `PORT` | `3000` | Listen port |
| `LITEROUTER_DB` | `literouter.db` | SQLite database path. **Set this to `/app/data/literouter.db`** when running the container — the default resolves to `/app/literouter.db`, outside the mounted volume |

## Usage

1. Log in to the admin UI → **Channels** to add upstream channels:
   - **OpenAI URL**: the **full** address of an OpenAI-protocol-compatible endpoint, including the version path (e.g. `https://api.openai.com/v1`, or Volcengine Ark's `https://ark.cn-beijing.volces.com/api/plan/v3`). The gateway appends `/chat/completions` and `/models` to it
   - **Anthropic URL** (optional): the **full** address of an Anthropic-protocol-compatible endpoint (e.g. `https://api.anthropic.com/v1`, or Volcengine Ark's `https://ark.cn-beijing.volces.com/api/plan`). The gateway appends `/v1/messages`
   - **Models**: enter manually (comma-separated), or click "Fetch models" to pull from the upstream `/v1/models`; `*` matches any model
   - **Enabled**: toggle off to keep the credentials stored but exclude the channel from routing (same effect as deleting it, but reversible)
2. **Tokens** — create internal keys (`sk-` prefixed), with optional RPM limits and daily token limits
3. (Optional) **Mappings** — configure routing rules: client model name → a list of upstream models (forwarded in order, with failover), fully transparent to clients
4. Point your internal services' SDK `base_url` at this gateway, with an internal key as `api_key`:

```python
from openai import OpenAI
client = OpenAI(
    base_url="http://your-host:3000/v1",
    api_key="sk-xxxx",  # internal token
)
resp = client.chat.completions.create(model="gpt-4o", messages=[...])
```

## API

Public (requires an internal token):

| Endpoint | Description |
|---|---|
| `POST /v1/chat/completions` | OpenAI protocol, supports `stream: true` |
| `POST /v1/messages` | Anthropic Messages protocol, supports streaming |
| `GET /v1/models` | Merged model list of all enabled channels |

Admin UI (Bearer session):

| Endpoint | Description |
|---|---|
| `POST /api/login` | Admin login |
| `GET/POST /api/channels`, `PUT/DELETE /api/channels/:id` | Channel management |
| `POST /api/channels/fetch-models` | Fetch model list from upstream |
| `POST /api/channels/test-model` | Test channel availability |
| `GET/POST /api/tokens`, `PUT/DELETE /api/tokens/:id` | Token management |
| `GET/POST /api/mappings`, `PUT/DELETE /api/mappings/:id` | Model routing management |
| `GET /api/logs` | Call logs |
| `GET /api/usage` | Usage statistics |

---

[Changelog](CHANGELOG.md) · [Releases](https://github.com/PandaJiuJiu/LiteRouter/releases)
