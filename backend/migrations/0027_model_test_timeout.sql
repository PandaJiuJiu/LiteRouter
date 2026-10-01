-- 模型测试（/api/channels/test-model）的单次探测超时。
--
-- 后台共享的 http client 允许 600s —— 那对真实转发是对的，但模型卡片是
-- 「点一下等结果」的界面：上游挂起时卡片会转十分钟。默认给 10s，
-- 上游慢的话改这一行，不用重新编译。
--
-- 读取方 admin::model_test_timeout，缺失/非数字/0 都回落到这个默认值。

INSERT OR IGNORE INTO settings (key, value) VALUES ('model_test_timeout_secs', '10');