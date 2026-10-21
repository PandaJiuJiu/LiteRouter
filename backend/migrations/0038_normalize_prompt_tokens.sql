-- 把 prompt_tokens 归一化成"输入总量"的口径。
--
-- 背景：两个协议对"这次请求的输入有多大"的说法不一样，而差异藏在字段名里。
--
--   OpenAI 的 prompt_tokens 是整个 prompt，缓存命中的部分已经含在里面；
--   Anthropic 的 input_tokens 只是【没命中缓存的那部分】，命中和写入的量
--     放在旁边的 cache_read_input_tokens / cache_creation_input_tokens。
--
-- 之前 logs.prompt_tokens 存的是上游的原值，于是同一列在不同协议下含义
-- 不同：Anthropic 渠道存的是余量，OpenAI 渠道存的是全量。聚合查询没法直接
-- 比较两条路径，"输入"这一列在用量页面上并排放着也是误导。
--
-- 现在解析层把两边都翻译成统一口径，prompt_tokens 一律是"输入总量"（缓存
-- 命中 + 缓存写入 + 未命中），所以历史数据也要跟着换算——原始数据都在库里，
-- 可以精确重算，不需要从别处回捞。
--
-- 判据一（哪些行要换算）："上游是哪个协议"。
--   convert = 'to_anthropic'                  → 上游说 Anthropic
--   protocol = 'anthropic' 且 convert = 'none' → 客户端和上游都是 Anthropic
-- 其余（to_openai，或客户端是 openai 时的 none）上游说的是 OpenAI，它的
-- prompt_tokens 本来就是全量，不用动。
--
-- 注意 convert='none' 不等于 Anthropic：它表示"客户端协议 == 上游协议"，
-- 所以一个 OpenAI 客户端打到一个只有 OpenAI 端点的渠道，也是 none，那行的
-- prompt_tokens 同样不能加缓存。
--
-- prompt_tokens 一律要加缓存：Anthropic 上游报的从来是余量，不管这行的
-- total 是按哪个公式写的。
--
-- total_tokens 要分两种：修复之前 total 是 prompt + completion（不含缓存），
-- 修复之后是输入总量 + 输出（含缓存）。"total = prompt + completion"正好
-- 区分这两批数据——旧行满足这个等式，新行不满足。旧的要一起补上缓存，
-- 新的已经算过了，不能再加一次。
--
-- 两条换算必须在同一条 UPDATE 里用 CASE 算：如果分成两条，第一条改完
-- prompt_tokens 之后，第二条的判据就不再成立。
UPDATE logs SET
    prompt_tokens = prompt_tokens + cache_read_tokens + cache_creation_tokens,
    total_tokens = CASE
        WHEN total_tokens = prompt_tokens + completion_tokens
            THEN total_tokens + cache_read_tokens + cache_creation_tokens
        ELSE total_tokens
    END
WHERE (convert = 'to_anthropic' OR (protocol = 'anthropic' AND convert = 'none'));
