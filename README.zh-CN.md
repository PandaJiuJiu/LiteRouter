简体中文 | [English](README.md)

# LiteRouter

一个轻量级的 LLM API 网关。将多个上游 LLM 服务（OpenAI / Claude / 各类中转站等）聚合为一个统一地址，向下游使用方签发独立的访问密钥，并提供模型路由、多渠道故障转移与用量统计能力。单个 Rust 二进制 + SQLite，无外部依赖，开箱即用。

## 功能特性

- **统一接入**：只对外暴露 `http://your-host:3000/v1` 一个地址，同时兼容两种协议：
  - `POST /v1/chat/completions` — OpenAI 协议（`Authorization: Bearer sk-...`）
  - `POST /v1/messages` — Anthropic Messages 协议（`x-api-key: sk-...`）
  - 均支持流式（SSE）透传
- **渠道管理**：上游渠道（OpenAI / Claude / 各类中转站）的密钥只保存在网关，不对外暴露；支持 OpenAI / Anthropic 双协议地址（同一个 key 双协议的渠道如火山方舟，两个地址都填即可）
- **渠道路由**：按请求中的 `model` 自动路由到支持该模型的上游渠道；支持通配符 `*`
- **协议转换**：客户端协议与渠道不一致时自动双向转换（Anthropic Messages ⇄ OpenAI ChatCompletions），含流式与工具调用。例如 OpenAI 协议的渠道可直接服务 Claude Code（Anthropic 协议客户端），无需任何配置
- **模型路由（Mappings）**：把客户端请求中的模型名（如 `my-model`）映射到一列上游真实模型 ID，可一对多并调整顺序；请求按列表顺序依次转发，前面的目标失败或无可用渠道时自动回退到后面的
- **多渠道故障转移**：上游出现传输错误或可重试状态码（408 / 429 / 5xx / 524）时自动切换到下一个候选渠道；不可重试的 4xx 原样返回给客户端
- **令牌与配额**：为自己的服务 / 使用方签发 `sk-` 开头的 key，支持按令牌的 RPM 限额与每日 token 用量限额（超限返回 429）
- **用量统计与调用日志**：记录每次请求的渠道、模型、状态码与 prompt / completion / total tokens，后台提供用量看板
- **客户端断连取消**：客户端断开连接时自动取消上游请求，不浪费上游配额
- **免运维**：SQLite 存储，无外部依赖，数据库 schema 自动迁移

## 技术栈

- 后端：Rust（axum + sqlx/SQLite + reqwest）
- 前端：Vue 3 + Vite + Element Plus

## 部署

使用 Docker Compose 一键部署：

```bash
git clone https://github.com/qihangkong/LiteRouter.git
cd LiteRouter
docker compose up -d --build
```

启动后访问 `http://your-host:3000`，系统会引导你创建第一个管理员账号。

说明：

- 镜像为三阶段构建（前端 → 后端 → 运行时），最终基于 `alpine:3.20`，无外部依赖
- SQLite 数据持久化在 `./data` 目录
- `docker-compose.yml` 依赖同目录的 `Dockerfile`，两者均无需修改即可使用
- 老版本 Docker 用 `docker-compose`（带连字符）代替 `docker compose`

### 账号体系

- 首次访问会自动跳转到 **初始化向导**，创建管理员账号（密码至少 8 位）
- 管理员可在「用户管理」中创建普通用户、重置密码、提升/降级角色、删除账号
- **渠道** 与 **模型路由** 是后台基础设施，由管理员统一管理，对所有账号共享
- **令牌** 每个用户独立创建和管理；管理员可见全部并可代建
- **调用日志** 与 **用量统计** 按用户自动过滤，管理员可见全部
- 至少保留一个管理员账号，防止锁死

### 通过环境变量预设初始管理员（可选）

如果不想走网页初始化向导，可以设置 `ADMIN_PASSWORD`，首次启动时会自动创建一个名为 `admin` 的管理员账号；后续修改密码请直接在网页上操作。

```bash
ADMIN_PASSWORD=your-password docker compose up -d --build
# 然后用 admin / your-password 登录
```

### 修改端口

宿主端口由 `docker-compose.yml` 的 `ports` 映射决定，已支持用 `PORT` 环境变量覆盖（容器内固定监听 3000）：

```bash
PORT=8080 docker compose up -d --build   # 用 http://your-host:8080 访问
```

注意这里的 `PORT` 只影响**宿主侧**端口映射；容器内进程监听端口无需修改。改回默认只需去掉 `PORT=` 前缀，重新 `up -d` 即可。

Docker Hub 不可达的网络可通过 build arg 换源：

```bash
docker compose build --build-arg REGISTRY=docker.io/library
```

### 环境变量

| 变量 | 默认值 | 说明 |
|---|---|---|
| `PORT` | `3000` | 监听端口 |
| `ADMIN_PASSWORD` | （空） | 可选：首次启动时自动创建 `admin` 账号并使用该密码；不设置则走网页初始化向导 |
| `LITEROUTER_DB` | `literouter.db` | SQLite 数据库路径（compose 已指向持久卷 `/app/data/literouter.db`） |

## 使用流程

1. 登录后台 → **渠道管理** 添加上游渠道：
   - **OpenAI URL**：兼容 OpenAI 协议的**完整地址**，含路径版本（如 `https://api.openai.com/v1` 或火山方舟的 `https://ark.cn-beijing.volces.com/api/plan/v3`）。网关在此基础上追加 `/chat/completions`、`/models`
   - **Anthropic URL**（可选）：兼容 Anthropic 协议的**完整地址**（如 `https://api.anthropic.com/v1` 或火山方舟的 `https://ark.cn-beijing.volces.com/api/plan`）。网关追加 `/v1/messages`
   - **模型**：可手填（逗号分隔），或点「自动获取模型」从上游 `/v1/models` 拉取；填 `*` 表示匹配任意模型
   - **启用**：关闭后保留凭证但渠道不参与路由（与删除等价，但可恢复）
2. **令牌管理** 创建内部 key（`sk-` 开头），可按需设置 RPM 限额与每日 token 限额
3. （可选）**模型路由** 配置映射规则：客户端模型名 → 一列上游模型（按顺序转发、失败回退），对客户端完全透明
4. 内部服务把 SDK 的 `base_url` 指向本网关，`api_key` 用内部 key：

```python
from openai import OpenAI
client = OpenAI(
    base_url="http://your-host:3000/v1",
    api_key="sk-xxxx",  # 内部令牌
)
resp = client.chat.completions.create(model="gpt-4o", messages=[...])
```

## API

对外（需内部令牌）：

| 端点 | 说明 |
|---|---|
| `POST /v1/chat/completions` | OpenAI 协议，支持 `stream: true` |
| `POST /v1/messages` | Anthropic Messages 协议，支持流式 |
| `GET /v1/models` | 合并所有启用渠道的模型列表 |

管理后台（Bearer session）：

| 端点 | 说明 |
|---|---|
| `POST /api/login` | 管理员登录 |
| `GET/POST /api/channels`、`PUT/DELETE /api/channels/:id` | 渠道管理 |
| `POST /api/channels/fetch-models` | 从上游拉取模型列表 |
| `POST /api/channels/test-model` | 测试渠道可用性 |
| `GET/POST /api/tokens`、`PUT/DELETE /api/tokens/:id` | 令牌管理 |
| `GET/POST /api/mappings`、`PUT/DELETE /api/mappings/:id` | 模型路由管理 |
| `GET /api/logs` | 调用日志 |
| `GET /api/usage` | 用量统计 |
