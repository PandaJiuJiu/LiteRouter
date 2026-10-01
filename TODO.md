# DONE

四项都已完成（2026-10-01）。

## Module test

1. ~~模型test功能显示 Available~~ → **已改成显示请求耗时**。后端每个协议探测返回 `{ ok, ms }`，前端按图标/颜色表示是否可用，旁边显示耗时（<1s 用 `842 ms`，≥1s 用 `2.50 s`）。绿色对勾/红色叉号 + tooltip 的错误信息都保留，「可用」这个词去掉了 —— 它在图标上已经说过一遍。
   `backend/src/admin.rs`（`probe`）/ `frontend/src/views/Models.vue`（`statusText`）

2. ~~模型test加超时~~ → **已加，默认 10s**。超时按普通失败返回，错误信息是 `请求超时（> 10s）`，显示在 tooltip 里，不会把接口打成 500。
   超时值不写死：每次探测前从 `settings` 表读 `model_test_timeout_secs`，读不到/非数字/0 都回落到 10s。上游慢的话直接改库，不用重新编译。迁移 `0027_model_test_timeout.sql` 预置默认值。
   `backend/src/admin.rs`（`model_test_timeout`）

## Setting Page

1. ~~一行一个设置~~ → **已改**。左右两栏（左标题、右控件，160px/200px 定宽），当前只有 Language 一行，控件是下拉选择。以后加设置就是多一个 `.row`。
   `frontend/src/views/Settings.vue`

2. ~~移除用户上拉菜单里的语言设置~~ → **已移除**。连带删掉了 `lang:` 菜单项、`onLanguageCommand`，以及失效的 `LANGUAGES`/`locale`/`switchLanguage`/`Select` 导入和 `.lang-active` 样式。Settings 是唯一的语言切换入口。
   `frontend/src/views/Layout.vue`

## 顺带做的

- 「全部测试」跳过已停用的模型。停用的模型不参与路由，测它没参考价值，还会把等待时间按停用数量线性拉长；跳过的卡片保持「尚未测试」，不会误显示成红色失败。按钮的 `:disabled` 也跟着改，全部停用时按钮才真的禁用。

## 测试

- 新增 `backend/tests/model_probe.rs`（8 个用例，wiremock 起真实上游）
- 新增 `frontend/tests/models_view.spec.js`（9 个用例）
- 更新 `frontend/tests/layout.spec.js`、`frontend/tests/settings_view.spec.js`

`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --all-targets`、`npm test` 均通过。

## 待定

模型测试的超时目前只在 settings 表里可改，Settings 页上还没露出这一行。等 Settings 页有第二、第三个设置时再加。