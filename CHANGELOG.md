# 更新日志

本文件记录 LiteRouter 的版本变更。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

镜像：`ghcr.io/pandajiujiu/literouter`

---

## [0.0.19] - 2026-10-22

自 v0.0.18 起共 10 个提交，聚焦用量统计的口径统一与客户端请求的如实转发。

### 新增

- **用量页新增缓存命中统计**：新增「缓存命中 tokens」与「缓存命中率」两列，四个用量维度（按 Token / 按 Model / 按 Channel / 按日）同步展示。缓存命中的 token 按折扣计价，一个从不提及缓存的用量报表无法回答"这单账单里有多少是缓存省下来的"，命中率则直接说明提示词缓存到底有没有生效
- **协议间统一的 token 口径**：两个协议用不同的名字报同样的量，差异藏在字段名里——OpenAI 的 `prompt_tokens` 是整个 prompt（含缓存命中），Anthropic 的 `input_tokens` 只是**没命中缓存**的部分，缓存量在旁边单独的计数器里。网关过去原样存上游的值，同一列在不同协议下含义不同，聚合查询无法直接比较两条路径。现在解析层做翻译而非拷贝，下游统一用一套词汇；新增第三种协议只是解析器多一条分支

### 修复

- **缓存与推理 token 一律丢失其一**：解析器只认得每个计数器的一种拼写，两条路径各丢一半——走 OpenAI 形态上游的请求（12,718 次）全部记为零缓存命中，而上游明明在每次调用报告 25 万缓存 token；走 Anthropic 形态上游的请求（10,359 次）全部记为零推理 token。两种拼写现在都读，取较大值
- **总 token 漏算缓存**：一次读取了 333,773 缓存 token 的请求被记为只消耗了 1,429。总 token 过去是 `prompt + completion`，而 Anthropic 的 `input_tokens` 只是未命中缓存的余量，缓存部分从未计入。这不只是个显示数字——日配额和用量聚合都按 `total_tokens` 求和，意味着一条走原生 Anthropic 渠道的请求只按实际消耗的 1/56 计数，配额实际上没有被执行
- **不再向 Anthropic 上游注入 `thinking: disabled`**：网关过去无条件地要求所有上游"不要思考"。这个标记是为 Claude Code 的短辅助调用引入的（其推理会吃掉整个 `max_tokens` 预算并返回 `content: null`），但"中转站会忽略未知字段"的假设只在 OpenAI 方言成立——Anthropic 方言更严格，MiniMax-M3.1-Flash 要求 adaptive thinking，直接以 400 拒绝 `thinking.type=disabled`；火山引擎的原生 Anthropic 端点同样拒绝，修复前累计 10,901 次连续 400、零成功。这两个渠道此前"能用"只是因为故障转移掩盖了每一次失败。现在透传渠道只改写 `model`，其余请求体逐字节原样转发
- **透传客户端的 `anthropic-beta` 请求头**：这个头是 beta 特性的开关（上下文管理、压缩、fast mode、服务端回退、思考展示更新），没有它 beta 特性就是哑的。丢弃它是在"隐藏客户端身份"时被顺带删掉的——但它不是身份头，关联 ID 已经足以让同一账号的并发会话无法区分。客户端带 `context_management` 字段时上游却从未被告知要遵守它。现在该头在上游说 Anthropic 时完整透传（多个 beta 标志可逗号分隔），OpenAI 形态的上游不发送
- **协议转换时保留客户端的思考意图**：请求转换器从零重建请求体，只搬运认识的字段，Anthropic 的思考控制不在其中——客户端说明想要多深的思考，到达 OpenAI 上游时完全没有这个指示，上游只好用自己的默认值。这不是罕见情况：Claude Code 每次请求都带 `thinking` 和 `output_config.effort`。现在两个方向都跨协议携带该意图，客户端没说的则一个字段都不加（替客户端挑一个思考等级，和当初注入 `thinking: disabled` 是同一种错误，只是失败得更安静）
- **用量页「今日」按本地零点计数**：此前用 `now - 86400`（滚动 24 小时窗口）且按 UTC 零点分组。现在从进程时区的本地零点开始，按日分组也按本地零点，响应带 `tz_offset_secs`，前端按后端计数所用的同一时区渲染日期
- **调试捕获记录真正发上线的请求头**：捕获写的是一份手写的"预期发送"头清单（`content-type`、`authorization`，上游说 Anthropic 时再加 `anthropic-version`）——是猜的，而且两头都撒谎：声称发了实际被丢掉的 `anthropic-beta`，也声称发了实际没有的 `anthropic-version`。现在请求先构建再发出，头从构建好的请求上读取；凭据在写入时脱敏

### 其他

- **数据库迁移 `0038_normalize_prompt_tokens`**：把历史上 Anthropic 形态行的 `prompt_tokens` 换算成"输入总量"口径。原始数据都在库里，可精确重算，无需从别处回捞。判据是转换模式配合客户端协议——`none` 只表示"客户端协议 == 上游协议"，所以 OpenAI 客户端打 OpenAI 渠道也是 `none`，那一行不该加缓存
- **日志详情页标签列不再折行**：两张表的首列宽度统一为固定 148px，标签改用 API 里的真实字段名（x-request-id、x-litrouter-request-id、cache_read、cache_creation、reasoning）——这正是管理员在日志行里会 grep 的名字。原先的说明文字移到数值旁的提示里，信息没有丢失
- **发布流程固化为项目级 skill**：`.claude/skills/release/SKILL.md` 记录完整的发版流程（版本号同步、CHANGELOG 写法、门禁、tag 与镜像发布、Release 页创建），并写明版本号必须三处一致——本次发布同时终结了历史遗留的 tag 与产品版本号漂移（v0.0.18 实际装的是 0.0.16）
- **本次起 tag 与产品版本号一致**：tag 即版本号（`v0.0.19` 装 `0.0.19`），设置页显示的版本与镜像标签从此对得上

---

## [0.0.16] - 2026-10-15

自 v0.0.10 起共 7 个提交，聚焦 CI 发布流水线与 GHCR 自动化。

### 新增

- **CI 自动发布 Docker 镜像到 GHCR**：`.github/workflows/ci.yml` 新增 `release` job，推送 `v*` 格式 tag 时自动构建镜像并推送到 `ghcr.io/pandajiujiu/literouter`（同时打 `latest`）。后端 / 前端 / Docker 冒烟测试全部通过后才会发布
- **GHCR 包关联仓库**：Dockerfile 新增 `org.opencontainers.image.source` 标签指向 `PandaJiuJiu/LiteRouter`，使镜像包直接关联并显示在仓库的 Packages 页面，而非游离的用户级包

### 修复

- **测试稳定性**：
  - `a_converted_stream_captures_the_upstream_bytes_too` 在异步 debug 捕获写盘完成前读取 `resp.json`，补齐 `wait_for_debug` 等待
  - 修正 `relay_contract.rs` import 顺序（cargo fmt）
  - 修正 `wait_for_debug` 的导入方式（直接导入调用，消除 clippy `unused_imports`）

### 其他

- **GHCR 仓库名统一为小写**：`PandaJiuJiu` 含大写字母，GHCR 要求仓库名全小写，镜像路径统一为 `ghcr.io/pandajiujiu/literouter`
- 发布流程：打 tag（`git tag v0.0.17 && git push origin v0.0.17`）→ CI 自动测试并发布，无需手动推镜像

---

## [0.0.10] - 2026-10-14

自 v0.0.9 起共 8 个提交，聚焦配置可移植性与工程基建。

### 新增

- **支持根目录 `.env` 统一配置端口**：`run.sh` 自动加载根目录 `.env`，其中的 `PORT`、`LITEROUTER_FRONTEND_PORT`、`LITEROUTER_DB` 同时作用于开发前后端；docker compose 原生读取同一份 `.env`。命令行参数 / 显式环境变量优先级更高
- **上游请求 ID 记录**：每个 hop 记录上游自己的请求 ID（`x-request-id` 等），失败排查时可跨系统关联

### 变更

- **开发端口迁移到高位**：后端默认 12000、前端默认 12001，避免与机器上常见服务（3000/5173/8080）冲突；`vite.config.js` 与 `Dockerfile` / `docker-compose.yml` 默认值同步对齐
- **Docker 默认端口同步为 12000**：`ENV PORT`、`EXPOSE` 与 compose 端口映射统一从 3000 改为 12000，并支持 `.env` 的 `PORT` 覆盖

### 修复

- **clippy `result_large_err`**：配置备份的三个 handler 返回 `Result<_, Response>` 触发 lint，补充 `#[allow]`（与文件内既有辅助函数一致）

### 其他

- **恢复 GitHub Actions CI**：新增 `.github/workflows/ci.yml`，三个并行 job（后端 fmt/clippy/test、前端 i18n 检查/单测/构建、Docker 构建+冒烟测试），推送 `main` 与 PR 时触发
- **添加 MIT 许可证**
- 修复两个测试与异步 debug 捕获写入之间的竞态

---

## [0.0.9] - 2026-10-10

自 v0.0.8 起共 10 个提交，聚焦流式中继的正确性与日志实时性。

### 新增

- **日志详情实时跟随调用链**：每个 hop 一开始就写入 `log_attempts`（`status_code=0`），落定后更新同一行，并通过 SSE 广播不断增长的调用链。请求开始时推送 `pending` 事件，每次 hop 状态迁移再推一次，详情页可渲染仍在飞行中的 hop，请求结束后自动重新拉取完整行
- **日志列表标识「进行中」**：待办行（`status_code=0`）使用暖色背景一眼可辨，状态下拉将 0 标为「进行中」而非裸的 0
- **用量页柱状图**：在表格旁用 SVG 绘制当前维度的柱状图，带指标选择器（默认总 token）与紧凑纵轴；数据直接取自现有 `/api/usage`，切换 tab / 指标无需重新请求
- **设置页显示系统版本**：`/api/setup-status` 返回 `CARGO_PKG_VERSION`，设置页卡片底部以只读行展示

### 修复

- **流式中继保真转发**（4 处缺陷）：
  - `OpenAiToAnthropicStream` 在前一个 block 尚未关闭时就发出新的 `content_block_start`，违反 Anthropic 一次只开一个 block 的约束。修复：开 tool block 前先关 text/thinking block，开 text 前先关 thinking，结束时按升序关闭所有仍打开的 block；`ToolSlot` 增加 `closed` 标记，已关闭的 block 不会被重新打开或重复关闭
  - Anthropic 的 `thinking` 内容在 OpenAI 侧被丢弃。修复：流式 `thinking_delta` 转发为 `reasoning_content`，非流式的 thinking block 也映射为 `reasoning_content`
  - 上游在终止符前关闭的流，客户端看起来却像正常结束。修复：新增 `SseConverter::mark_truncated`，EOF 时若未收到终止事件，`OpenAiToAnthropicStream` 以 `event: error` 收尾而非 `message_stop`，`AnthropicToOpenAiStream` 在正常尾部与 `[DONE]` 前插入 error chunk
  - `passthrough_stream` 的扫描从按行改为按帧，只在 SSE 空行处派发；原实现只剥离行尾 `\r`，留下的 `\n` 让空行派发永不触发，usage / 终止 / error 检测全部静默失效
- **响应前被取消的请求得到结算**：客户端在任何响应体存在前挂断时，hyper 会在上游 await 处丢弃 handler future，缓冲路径的内联 `log_request` 与流体的 Drop 都不会执行，预建的待办行一直停在 `status_code=0`。修复：`PendingLogGuard` 在 `insert_pending_log` 之后创建，靠自身 Drop 以 `499` / `client_aborted = 1` 结算，并由 `WHERE status_code = 0` 兜底，绝不覆盖真实结果；任何写入真实行的路径都会解除它
- **日志列表 SSE 状态过滤**：`if (fp.status)` 会让状态 0 直接跳过过滤，且字符串与数字用 `!==` 比较。修复：按分支逐项检查，并在过滤期间已落定、不再匹配的行会从列表移除

### 其他

- `run.sh` 新增 `restart` 子命令：停止两个 dev 进程，等待端口释放后重启（等价于手动 `stop && start`）
- 停止跟踪 `tmp/` 并加入 `.gitignore`

---

## [0.0.8] - 2026-10-09

自 v0.0.7 起共 3 个提交，专注于日志系统增强。

### 新增

- **请求 ID 追踪**：日志表新增 `request_id` 字段，请求开始时即写入待办行（`status_code=0`），完成后更新。流式请求通过 SSE 推送 `pending` / `log` 事件，前端去重展示实时状态
- **日志列表显示请求 ID**：新增 Req ID 列，方便关联日志详情页
- **流式请求记录端到端耗时**：原仅记录 TTFT（首包时间），现改为完整流结束后的总耗时，日志详情提示更新为「请求总时间」

### 修复

- **待办行清理机制**：后台任务每 30 分钟扫描并清理超过 30 分钟的卡住待办行，防止异常断连导致脏数据堆积

---

## [0.0.7] - 2026-10-09

自 v0.0.6 起 1 个提交，紧急修复。

### 修复

- **Mappings 保存后 targets 被改写为不可读形式**：`TargetEntry` 的文档注释声称
  `#[serde(untagged)]` 但实际从未添加该属性，serde 默认的外部标签化让
  `encode_targets` 存成 `{"Obj":{"channel":…,"model":…}}`——这一形态
  `deserialize_targets` 会静默丢弃，导致每个 target 都塌缩到 legacy
  `target_model` 兜底（显示「全部模型」且无渠道）。修复：
  - 补上真正的 `#[serde(untagged)]`，存储形态可正常 round-trip
  - 读取时解包旧数据里的 `{"Obj":{…}}` 包装，已损坏的存量 DB 无需数据修复即可恢复
  - 新增 encode→parse round-trip 与解包逻辑的回归测试

---

## [0.0.6] - 2026-10-09

自 v0.0.5 起共 34 个提交。

### 新增

- **日志页增加 TTFT 指标**：新增 `logs.ttft_ms` / `log_attempts.ttft_ms` 字段，
  记录流式请求从发起到收到首个 token 的耗时（毫秒），在日志详情页展示
- **熔断器支持 Retry-After 提示**：上游返回 429 时携带 `Retry-After` 响应头，
  优先使用该值作为退避时长（上限 600s），而非自行计算指数退避
- **用量统计图**：前端用量页新增柱状图展示，直观对比各时段的 token 消耗
- **流截断原因诊断**：当上游在协议终止符（`message_stop` / `[DONE]`）前断开连接时，
  `error` 字段记录详细原因——`'upstream closed before stream end'` / `'upstream stream error'` /
  `'upstream transport error'`，无需查看 debug capture 即可定位 provider 问题

### 修复

- **streaming 响应 `usage: null` 导致 0 token**：云知声等 provider 的流式 chunk 带 `"usage": null`，
  原代码将其视为「无 usage 字段」返回 `None`，merge 逻辑跳过这些 chunk 导致 log 显示 0 tokens。
  修复：`usage_from_sse_payload()` 检测 `null` 后返回 `Some(Usage::default())`
- **Anthropic→OpenAI 流式转换的 usage 合并**：Anthropic 流将 input/output tokens 拆分到不同事件，
  原来 wholesale replace 会被最后一帧的 output only chunk 覆盖 prompt tokens。
  修复：改为字段级合并
- **OpenAI 请求未带 `include_usage` 导致流式请求记录 0 tokens**：OpenAI 端点需要显式请求
  `stream_options: { include_usage: true }` 才会返回 usage，原 convert 漏掉了

### 重构

- **后端**：
  - `convert.rs` 拆分为 `convert/` 目录（`mod.rs` + `openai.rs` + `anthropic.rs` + `streaming.rs`）
  - `mappings.rs` 提取 `TargetEntry` + `parse/encode/clean_targets` 工具函数，解决 proxy 和 config_backup 的循环依赖
  - `db.rs` 统一 `deserialize_use_proxy` 消除三处重复定义
  - `settings.rs` 三组 proxy UPSERT 合并为循环
  - `users.rs` 提取 `has_other_admin` 复用逻辑
  - `proxy_enabled` 字段改名为 `proxy_on` 避免遮蔽
  - 配置备份三组 `free_channel/token/alias` 循环合并为单个通用循环
- **前端**：设置入口从侧边栏移至用户下拉菜单；模型管理页支持拖拽排序

### 移除

- **GitHub Actions CI**：fmt / clippy 改为本地手动检查，加快迭代速度

---

## [0.0.5] - 2026-10-04

自 v0.0.4 起共 19 个提交。

### 新增

- **全局 HTTP/HTTPS 代理**：设置页新增「代理」段，拆成两行独立配置——
  - **代理服务器地址**（`proxy_host` / `proxy_port`）：只是地址，填了**不会**隐式启用代理
  - **全局启用开关**（`proxy_enabled`）：独立的总开关（默认关），打开后所有渠道都走代理

  渠道级的 `use_proxy` 开关与全局开关是 **OR** 关系，互不依赖：全局开关是一键让所有渠道走代理，渠道开关则只管自己（全局关掉时，单独勾了照样走代理）。最终判定为 `(全局开关 OR 渠道 use_proxy) AND 代理服务器已配置`。`AppState::proxy_active_for(channel_use_proxy)` 承担这一判定，六个调用点（relay 候选链、渠道测试、半开探测）各自传入对应渠道的 `use_proxy`——failover 时每个候选各判各的；渠道列表与模型页的「代理」标签也基于此判定，避免出现「渠道页说在走代理、实际却在直连」的不一致

  `PUT /api/settings/proxy` 的三个字段全部改为可选的部分更新（`host` / `port` / `enabled` 缺省即保持原值），切换开关不再顺手覆盖地址。设置页代理段的界面提示同步说明两个开关的 OR 关系，渠道页 `use_proxy` 开关下也补了一行说明——这是新语义在界面上唯一可见的地方

  迁移 `0032_proxy_enabled` 默认值 0，保证已有实例在升级后不会突然开始走代理

### 修复

- **全局开关与渠道开关被错误耦合**：原先渠道的 `use_proxy` 只有在全局开关开启时才生效，两个本不相干的决策被绑在一起——管理员在渠道页勾了「使用代理」，还得再去设置页开总开关才有用；总开关一开，又会给所有渠道强加代理。判定改为 `(全局 OR 渠道) AND 已配置`（`proxy_effective()` 更名为 `proxy_active_for(channel_use_proxy)`），语义由「全局 && 配置」变成按渠道求值
- **死 provider 在客户端看到之前就 failover**：流式响应在 commit 之前先 peek 首条完整 SSE 帧（8 KiB / 1.5 s）。首帧是 `event: error` 或 OpenAI 风格 error envelope 时，hop 分类为 `InvalidBody` 并尝试下一候选；首帧是 content 时，peek 的字节作为响应体开头转发，后续流接续。否则一个只发 `event: error` 的流会落到客户端再无回退
- **流式响应里的 error 事件不再误记为成功**：SSE pump 扫描每一行，识别 `event: error`（Anthropic）与 `data:` 里携带 error envelope（OpenAI 风格 relay），hop 记为失败并喂给熔断 `Outcome::Failure(upstream_message)`。否则 relay 用 `200` + 立即死亡会让熔断器永远不退避
- **熔断跳过不再污染最终状态码**：被熔断器 skip 的候选在日志里是 hop，但不计入「所有候选都失败」的分母——它们从未真正打到上游，对下游如何失败没有发言权。错误信封按客户端协议成形：`/v1/messages` 返回 Anthropic 形态的 `{"type":"error","error":{…}}` 带 spec `error.type`，`/v1/chat/completions` 保持 OpenAI 形态
- **日志页**：流式失败但 HTTP 状态为 200 的请求不再以绿色显示
- **日志详情 · 上游响应捕获**：没有捕获就不显示那一段，有捕获也只剩一个「加载上游响应」按钮，没有标题/说明。捕获标志改为一次 `stat`（不读文件）
- **探针统一**：熔断器主动探测与渠道模型测试改用同一份 `probe.rs`，避免两套实现漂移
- **UI**：Models 页代理 / 已启用 / OpenAI 标签之间加间距；代理标签从蓝色改为 warning（橙）与 OpenAI 标签区分

### 数据库变更

- `0030_proxy_settings` — `settings` 新增 `proxy_host` / `proxy_port`
- `0031_channel_use_proxy` — `channels` 新增 `use_proxy INTEGER NOT NULL DEFAULT 0`
- `0032_proxy_enabled` — `settings` 新增 `proxy_enabled`（默认 `0`）

三个迁移都是 `INSERT OR IGNORE` / `ALTER TABLE ADD COLUMN` 语义，可安全重放。`proxy_enabled` 默认 0 是设计意图：升级时已有实例不会突然开始走代理

### 测试

后端 327 → 353 例，前端 172 → 175 例（i18n 351 → 360 key 双语齐平）：

- `relay_contract.rs` 新增 16 例：流式首帧 error 触发 failover、首帧 content 不丢字节、协议转换路径上的 peek、错误信封按协议成形、被熔断 skip 不污染最终状态（502 vs 429 vs 504）
- `manage_data.rs` 新增 10 例覆盖代理设置（普通用户不可见、默认未配置且未启用、配置地址不隐式启用、切 `enabled` 不破坏 host/port、`proxy_active_for` 四种组合、helper 回环）+ 4 例 `/api/logs/:id/debug` 的越权 / 无捕获 / 未登录 / 默认响应。其中 `proxy_state_reflects_effective_combination_in_state` 此前有两条断言在 OR 语义下自相矛盾（全局关闭后先断言该渠道不走代理，紧接着又断言同样条件的渠道走代理），测试当时是红的——按新语义重排为四种组合
- `logs_view.spec.js` 新增流式 200 不显示为绿色的用例；`log_detail.spec.js` 新增「无捕获则不渲染按钮 / 有捕获则渲染按钮」的用例

### 其他

- GitHub 仓库地址从 `qihangkong/LiteRouter` 迁移到 `PandaJiuJiu/LiteRouter`（README / 侧栏 GitHub 入口 / CHANGELOG 链接全部更新）。`use_proxy` 反序列化同时接受 int（0/1）和 bool，兼容 SQLite 返回 int 的场景

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

[0.0.19]: https://github.com/PandaJiuJiu/LiteRouter/releases/tag/v0.0.19
[0.0.16]: https://github.com/PandaJiuJiu/LiteRouter/releases/tag/v0.0.16
[0.0.10]: https://github.com/PandaJiuJiu/LiteRouter/releases/tag/v0.0.10
[0.0.5]: https://github.com/PandaJiuJiu/LiteRouter/releases/tag/v0.0.5
[0.0.4]: https://github.com/PandaJiuJiu/LiteRouter/releases/tag/v0.0.4
[0.0.3]: https://github.com/PandaJiuJiu/LiteRouter/releases/tag/v0.0.3
[0.0.2]: https://github.com/PandaJiuJiu/LiteRouter/releases/tag/v0.0.2
[0.0.1]: https://github.com/PandaJiuJiu/LiteRouter/releases/tag/v0.0.1