English | [简体中文](README.zh-CN.md)

# LiteRouter

A lightweight LLM API gateway. Aggregates multiple upstream LLM services (OpenAI / Claude / relay stations, etc.) behind a single unified endpoint, issues its own access keys to downstream clients, and provides model routing, multi-channel failover, and usage tracking. Single Rust binary + SQLite, no external dependencies, ready out of the box.

## Features

- **Unified access**: one endpoint at `http://your-host:3000/v1`, compatible with both protocols:
  - `POST /v1/chat/completions` — OpenAI protocol (`Authorization: Bearer sk-...`)
  - `POST /v1/messages` — Anthropic Messages protocol (`x-api-key: sk-...`)
  - Both support streaming (SSE) passthrough
- **Channel management**: upstream channel keys (OpenAI / Claude / relay stations) are stored only in the gateway and never exposed; supports dual-protocol URLs (for channels serving both protocols with the same key, such as Volcengine Ark, just fill in both URLs)
- **Channel routing**: routes requests to upstream channels that support the requested `model`; wildcard `*` supported
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

One-command deployment with Docker Compose:

```bash
git clone https://github.com/qihangkong/LiteRouter.git
cd LiteRouter
ADMIN_PASSWORD=your-password docker compose up -d --build
```

After startup, visit `http://your-host:3000` for the admin UI (default password `admin123`, change it via the `ADMIN_PASSWORD` environment variable).

Notes:

- The image is a three-stage build (frontend → backend → runtime), final stage based on `alpine:3.20`, no external dependencies
- SQLite data is persisted in the `./data` directory
- `docker-compose.yml` depends on the `Dockerfile` in the same directory; both work without modification. `ADMIN_PASSWORD` falls back to the default when unset
- On older Docker versions, use `docker-compose` (with hyphen) instead of `docker compose`

### Changing the port

The host port is determined by the `ports` mapping in `docker-compose.yml`, overridable via the `PORT` environment variable (the container always listens on 3000):

```bash
PORT=8080 docker compose up -d --build   # access at http://your-host:8080
```

Note that `PORT` only affects the **host-side** port mapping; the in-container process port needs no change. To revert to the default, drop the `PORT=` prefix and run `up -d` again.

If Docker Hub is unreachable, you can switch the registry mirror via build args:

```bash
docker compose build --build-arg REGISTRY=docker.io/library
```

### Environment variables

| Variable | Default | Description |
|---|---|---|
| `PORT` | `3000` | Listen port |
| `ADMIN_PASSWORD` | `admin123` | Admin UI password |
| `LITEROUTER_DB` | `literouter.db` | SQLite database path (compose points it at the persistent volume `/app/data/literouter.db`) |

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
