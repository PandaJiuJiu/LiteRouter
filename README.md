# Lite One API

一个轻量级的 LLM API 网关，参考 [one-api](https://github.com/songquanpeng/one-api) 的核心概念，但只保留最必要的功能：**渠道管理 + 内部令牌 + API 转发**。

## 适用场景

你拥有多个付费上游 API（OpenAI / Claude / 各类中转站等），希望对内提供**一个统一地址 + 一套内部 key**：
- 内部服务统一访问 `http://your-host:3000/v1/chat/completions`
- 网关按请求中的 `model` 自动路由到支持该模型的上游渠道
- 上游渠道密钥只保存在网关，不对外暴露
- 支持流式（SSE）透传

不做的功能（与 one-api 的差异）：多用户、计费配额、兑换码、模型重试/负载均衡等。

## 技术栈

- 后端：Rust（axum + sqlx/SQLite + reqwest）
- 前端：Vue 3 + Vite + Element Plus

## 快速开始

### 开发模式

```bash
# 后端（默认监听 :3000，SQLite 文件 lite-one-api.db）
cd backend
cargo run

# 前端（:5173，/api 与 /v1 已代理到后端）
cd frontend
npm install
npm run dev
```

打开 http://localhost:5173 ，默认管理员密码 `admin123`（环境变量 `ADMIN_PASSWORD` 可修改）。

### 生产部署

```bash
cd frontend && npm run build   # 产物输出到 frontend/dist
cd backend && cargo build --release
PORT=3000 ADMIN_PASSWORD=your-password ./target/release/lite-one-api
# 后端会自动托管 frontend/dist 静态文件
```

### 环境变量

| 变量 | 默认值 | 说明 |
|---|---|---|
| `PORT` | `3000` | 监听端口 |
| `ADMIN_PASSWORD` | `admin123` | 管理后台密码 |
| `LITE_ONE_API_DB` | `lite-one-api.db` | SQLite 数据库路径 |

## 使用流程

1. 登录后台 → **渠道管理** 添加上游渠道
   - **OpenAI URL**：兼容 OpenAI 协议的**完整地址**，含路径版本（如 `https://api.openai.com/v1` 或火山方舟的 `https://ark.cn-beijing.volces.com/api/plan/v3`）。网关在此基础上追加 `/chat/completions`、`/models`
   - **Anthropic URL**（可选）：兼容 Anthropic 协议的**完整地址**（如 `https://api.anthropic.com/v1` 或火山方舟的 `https://ark.cn-beijing.volces.com/api/plan`）。网关追加 `/v1/messages`（Anthropic API 所有端点都在 `/v1/` 下）
   - 同一个 key 双协议的渠道（如火山方舟）两个地址都填即可
   - **模型**：可手填（逗号分隔），或点"自动获取模型"从上游 `/v1/models` 拉取
2. **令牌管理** 创建内部 key（`sk-` 开头）
3. 内部服务把 OpenAI SDK 的 `base_url` 指向本网关，`api_key` 用内部 key：

```python
from openai import OpenAI
client = OpenAI(
    base_url="http://your-host:3000/v1",
    api_key="sk-xxxx",  # 内部令牌
)
resp = client.chat.completions.create(model="gpt-4o", messages=[...])
```

## API

内部（对外提供两种协议）：
- `POST /v1/chat/completions` — OpenAI 协议（`Authorization: Bearer sk-...`，支持 `stream: true`）
- `POST /v1/messages` — Anthropic Messages 协议（`x-api-key: sk-...`，支持流式）
- `GET /v1/models`

管理后台：
- `POST /api/login`、`/api/channels`、`/api/tokens`、`/api/logs`（Bearer session）

## Roadmap

- [ ] 渠道优先级 / 负载均衡与失败重试
- [ ] 模型重映射（model_mapping）
- [ ] 多用户与配额
