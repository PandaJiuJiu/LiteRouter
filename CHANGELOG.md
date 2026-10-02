# 更新日志

本文件记录 LiteRouter 的版本变更。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

镜像：`ghcr.io/qihangkong/literouter`

---

## [0.0.4] - 2026-10-02

自 v0.0.3 起共 17 个提交。

### 新增

- **配置备份 / 还原**：设置页新增加密导出与还原，三段（渠道 / 令牌 / 模型路由）可按行三选一，口令派生密钥加密成 `.lrbak` 文件；`users` 表刻意不可导出。管理员专属——文件里的 `api_keys` 与 `sk-…` 令牌在密文中是明文
- **熔断器历史**：熔断事件落库，新增「熔断器历史」页按时间倒序展示每个 (渠道, 模型) 的 tripped / re-tripped / recovered / reset 序列，含触发原因与退避时长。此前熔断器只活在内存里，进程重启即清空，运维无法回答"哪个模型什么时候熔断过、什么时候恢复"
- **日志耗时列**：日志列表新增请求耗时（秒）
- **日志筛选**：列表页接上 `/api/logs/filter-options`，时间 / 状态 / 渠道 / 令牌 / IP 五个下拉
- **日志详情 · 上游响应捕获**：新增 `GET /api/logs/:id/debug`，日志详情页按需加载捕获到的上游响应体，JSON 美化显示、失败回退原文，标注截断与字节数
- **模型管理**：新增侧栏入口与顶部工具条，工具条开关改为"只看已启用"
- **获取模型列表超时**：`fetch-models` 复用 `model_test_timeout_secs`（默认 10s），不再继承共享 client 的 600s
- **熔断手动探测**：`POST /api/breaker/probe-now` 立即探测所有熔断中的 (渠道, 模型) 组合，不等退避 cooldown 走完；熔断面板新增「立即探测」按钮
- **日志保留天数可配置**：从硬编码改为设置项下拉，落库到 `settings.log_retention_days`

### 修复

- **中继不再伪装成功**：`try_upstream` 对 2xx 按协议做形状校验（anthropic 需 `type=="message"`，openai 需 `choices[0].message`）。中转站用 `200 {"error":...}` 回答自己的内部故障时，此前被当作成功透传给客户端，现在和 4xx/5xx/transport error 一样走 failover。流式路径无法缓冲 body，改为只看 Content-Type：2xx 但 `application/json` 即判失败，且在客户端拿到任何字节之前拒绝
- **调试开关对非流式请求完全无效**：`AppState.debug_logging` 从 `bool` 改为 `AtomicBool`，每次从 state 读实时值
- **`resp.json` 永远 0 字节**：`passthrough_stream` 与 `converted_stream` 现在真的写 capture
- **SSE 协议违规**：`OpenAiToAnthropicStream::finish()` 在上游开了流却未发出任何 payload 就 EOF 时，吐出 `message_delta` + `message_stop` 而没有 `message_start`
- **失败请求无条件捕获响应体**：不再赌调试开关当时开没开。删除 `req.json`——从不存请求体是迁移 0014 的策略，现在做成结构性事实
- **用量页渠道维度**：过滤掉「全部候选均失败」的空渠道名分组（那种请求的 `channel_name` 刻意留空，不属于任何渠道）
- **侧栏**：移除「熔断器历史」菜单项（改从熔断面板进入）

### 数据库变更

- `0028_log_retention_days` — 新增 `settings` 中的 `log_retention_days`
- `0029_breaker_history` — 新增 `breaker_events` 表（append-only，按时间倒序查询）

两个迁移都是 `INSERT OR IGNORE` / `CREATE TABLE IF NOT EXISTS` 语义，可安全重放。

### 测试

后端 250 → 327 例，前端 138 → 172 例（i18n 351 key 双语齐平）：

- `relay_contract.rs` 新增 12 例：2xx 错误体触发 failover、混合 429+坏 200 落到 502（非 429）、非法 200 会熔断、关开关时失败请求仍落盘、成功请求关开关时不落盘、`req.json` 不存在、流式拒 JSON 200、超 256 KB 截断标注、两条流式路径 capture 非空
- `manage_data.rs` 新增 4 例覆盖 `/api/logs/:id/debug` 的越权 404 / 无捕获 200 / 未登录 401 / 默认响应未新增字段
- `stream_convert.rs` 新增 2 例钉住 `finish()` 的修复；`model_probe.rs` 新增 3 例（超时 / 正常 / 超时回落）
- 前端新增 `config_backup.spec.js`、`breaker_history.spec.js`、`logs_view.spec.js`、`log_detail.spec.js`

### 文档

- 重写部署章节，GHCR 预构建镜像作为推荐路径

---

## [0.0.3] - 2026-10-01

自 v0.0.2 起共 29 个提交。

### 新增

- **中英双语**：全站界面支持简体中文 / English。首次初始化的向导里选择语言，之后可在侧边栏用户下拉或「设置」页随时切换，偏好落库到 `settings.ui_language`，默认中文，既有实例不受影响
- **设置页**：新增「设置」页与侧边栏入口，集中管理界面语言
- **模型测试**：卡片上的 `Available` 标记改为展示实测耗时，并给单次探测加了超时（默认 10s，可改设置项、无需重新编译）；设置页改为一行一项的排版
- **熔断面板**：展示熔断触发的原因，不再只有一个「已熔断」标签

### 修复

- **令牌复制**：复制按钮此前依赖 `navigator.clipboard`，在非安全上下文（HTTP 访问、局域网 IP 等）下不可用，现回退到 `execCommand`；Key 默认以遮蔽形式显示
- **重复用户名 / 重复 alias**：重复的用户名与重复的模型路由 alias 现在返回 409，不再是 500
- **认证**：`password_hash` 为空时不得验证通过
- **登录**：语言落库改用归一化后的值；登出跳转移入 `finally`，跳转失败也不会卡住
- **i18n**：补上 `nav.tokens` 与 `tokens.unassigned` 两个缺失的 key
- **布局**：补全局 CSS reset，消除 100vh 页面多出的滚动条

### 数据库变更

- `0026_ui_language` — 新增 `settings` 中的 `ui_language`
- `0027_model_test_timeout` — 新增 `settings` 中的 `model_test_timeout_secs`

两个迁移都是 `INSERT OR IGNORE`，可安全重放。

### 测试

接入 GitHub Actions（`cargo fmt` / `cargo clippy -D warnings` / 后端测试 / i18n key 一致性 / 前端测试 / 前端构建，均为阻塞项），后端测试从 236 例扩到 250 例，前端从无到 138 例：

- 抽出 lib target 与 `build_router`，集成测试可在进程内挂载真实 `Router`（此前路由写在 `main.rs` 里，测不到）
- relay 契约测试 40 例，用 wiremock 充当真实上游
- 协议转换纯函数测试 60 例 + SSE 流式转换状态机 29 例
- 数据层测试 34 例（token / 日志 / 用量 / 设置）
- 前端 vitest 12 个套件 138 例：组件、路由守卫、axios 拦截器、共享 store
- `scripts/check-i18n.mjs`：零依赖的 key 一致性检查，防止两个语言包漂移

### 构建与重构

- 前端升级 vite 5 → 8、plugin-vue 5 → 6，`npm audit` 归零
- 抽出 `format.js` / `session.js` / 用量共享列，删掉无人引用的图与失效的垫片
- 新增 [CLAUDE.md](CLAUDE.md)，记录项目约定与若干易踩的坑

---

## [0.0.2] - 2026-09-30

自 v0.0.1 起共 29 个提交。

- 模型管理：拆分 enabled / disabled 两套持久化集合，停用卡片刷新后仍可见可恢复；卡片布局重设计；获取模型改为多选对话框；新增专用 models 更新接口
- 熔断器：内存化状态机、×2 退避封顶 600s、402 计入失败、skip 与失败分离；新增 (channel, model) 级主动探测
- 日志：详情页展示来源信息、客户端 IP、User-Agent；分页显示总数；按时间窗口筛选
- 调试日志：全局开关，开启时把请求/响应 body 写入 `data/debug_logs/`
- 侧边栏：用户区改为下拉菜单，新增项目 GitHub 入口
- 渠道：增加「官网」字段，名称显示可点击
- 后端：迁移至 `sqlx::migrate!()`，新增 13 份迁移文件 + CLAUDE.md
- 构建：Dockerfile 补回 `backend/migrations` 复制（sqlx 编译期宏需要），并配置 cargo rsproxy 镜像以应对 CN 网络
- UI：纯字标 logo 接入后台界面

数据库变更：`0022_breaker_linear_backoff`、`0023_log_attempts_skipped`、`0024_failed_count_backfill`、`0025_channel_disabled_models`

---

## [0.0.1]

首个版本。多账号体系、首次设置向导、渠道与模型路由、令牌与配额、日志与用量统计、协议转换、熔断器。

---

[0.0.4]: https://github.com/qihangkong/LiteRouter/releases/tag/v0.0.4
[0.0.3]: https://github.com/qihangkong/LiteRouter/releases/tag/v0.0.3
[0.0.2]: https://github.com/qihangkong/LiteRouter/releases/tag/v0.0.2
[0.0.1]: https://github.com/qihangkong/LiteRouter/releases/tag/v0.0.1