# 更新日志

本文件记录 LiteRouter 的版本变更。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

镜像：`ghcr.io/qihangkong/literouter`

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

[0.0.3]: https://github.com/qihangkong/LiteRouter/releases/tag/v0.0.3
[0.0.2]: https://github.com/qihangkong/LiteRouter/releases/tag/v0.0.2
[0.0.1]: https://github.com/qihangkong/LiteRouter/releases/tag/v0.0.1