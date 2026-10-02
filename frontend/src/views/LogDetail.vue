<template>
  <el-card v-loading="loading">
    <div class="header">
      <el-button text @click="back">{{ t('logDetail.back') }}</el-button>
      <span class="title">{{ t('logDetail.title', { id: log?.id }) }}</span>
    </div>

    <template v-if="log">
      <!-- 一眼扫到的状态条 -->
      <div class="status-bar">
        <el-tag :type="ok ? 'success' : 'danger'" size="large">{{ log.status_code }}</el-tag>
        <span class="latency" v-if="log.latency_ms > 0">
          {{ log.latency_ms.toLocaleString() }} ms
          <span class="hint">{{ t('logDetail.latencyHint') }}</span>
        </span>
        <el-tag v-if="log.stream" size="small" type="info">{{ t('logs.stream') }}</el-tag>
        <el-tag v-if="log.convert && log.convert !== 'none'" size="small" type="warning">
          {{ t('logDetail.convert', { convert: log.convert }) }}
        </el-tag>
        <el-tag v-if="log.failed_count > 0" size="small" type="warning" effect="plain">
          {{ t('logDetail.failedThenOk', { count: log.failed_count }) }}
        </el-tag>
      </div>

      <el-alert v-if="log.error" type="error" :title="log.error" :closable="false" show-icon />

      <!-- 基本信息 -->
      <section>
        <h3>{{ t('logDetail.sectionRequest') }}</h3>
        <el-descriptions :column="1" border>
          <el-descriptions-item :label="t('logDetail.field.token')">{{ log.token_name }}</el-descriptions-item>
          <el-descriptions-item :label="t('logDetail.field.protocol')">{{ log.protocol || '—' }}</el-descriptions-item>
          <el-descriptions-item :label="t('logDetail.field.clientIp')">{{ log.client_ip || '—' }}</el-descriptions-item>
          <el-descriptions-item :label="t('logDetail.field.time')">{{ formatTime(log.created_at) }}</el-descriptions-item>
          <el-descriptions-item :label="t('logDetail.field.modelRoute')">
            {{ log.request_model || '—' }} → {{ log.upstream_model || log.request_model || '—' }}
          </el-descriptions-item>
          <el-descriptions-item label="User-Agent">
            <span v-if="log.user_agent" class="ua-full">{{ log.user_agent }}</span>
            <span v-else class="hint">—</span>
          </el-descriptions-item>
        </el-descriptions>
      </section>

      <!-- 转发链路：一次请求 = 一行日志，内部的每一次上游尝试在这里按顺序展开 -->
      <section v-if="log.attempts && log.attempts.length">
        <h3>
          {{ t('logDetail.sectionChain') }}
          <span class="hint">
            {{ attemptSummary }}
          </span>
        </h3>
        <el-table :data="log.attempts" size="small" border :row-class-name="attemptRowClass">
          <el-table-column label="#" width="46">
            <template #default="{ row }">{{ row.seq + 1 }}</template>
          </el-table-column>
          <el-table-column prop="upstream_model" :label="t('logDetail.col.upstreamModel')" min-width="170" show-overflow-tooltip />
          <el-table-column :label="t('logDetail.col.channel')" min-width="130">
            <template #default="{ row }">{{ row.channel_name || '—' }}</template>
          </el-table-column>
          <el-table-column :label="t('logDetail.col.result')" width="110">
            <template #default="{ row }">
              <el-tag v-if="row.skipped" type="info" size="small" effect="plain">{{ t('logDetail.skipped') }}</el-tag>
              <el-tag v-else :type="row.ok ? 'success' : 'danger'" size="small">
                {{ row.ok ? t('logDetail.ok') : row.status_code === -1 ? t('logDetail.connFailed') : row.status_code }}
              </el-tag>
            </template>
          </el-table-column>
          <el-table-column :label="t('logDetail.col.latency')" width="90">
            <template #default="{ row }">
              <span class="num" :class="{ muted: row.skipped }">
                {{ row.latency_ms.toLocaleString() }} ms
              </span>
            </template>
          </el-table-column>
          <el-table-column :label="t('logDetail.col.error')" min-width="180" show-overflow-tooltip>
            <template #default="{ row }">
              <span v-if="row.error" class="err" :class="{ muted: row.skipped }">{{ row.error }}</span>
              <span v-else class="hint">—</span>
            </template>
          </el-table-column>
        </el-table>
        <p class="hint note">
          {{ t('logDetail.latencyNote') }}
        </p>
      </section>

      <!-- 捕获到的上游响应体。仅上游响应被落盘，请求体从不落盘
           （见 migration 0014）。失败请求自动捕获；健康请求需要打开 debug 开关。 -->
      <section>
        <h3>{{ t('logDetail.sectionDebug') }}</h3>
        <p class="hint">{{ t('logDetail.debugPrivacy') }}</p>
        <div v-if="debug.loading" class="hint">{{ t('common.loading') }}</div>
        <div v-else-if="debug.error" class="hint">
          {{ t('common.error') }}: {{ debug.error }}
        </div>
        <div v-else-if="debug.data === null">
          <el-button @click="loadDebug" size="small" :disabled="loading">
            {{ t('logDetail.debugLoad') }}
          </el-button>
        </div>
        <template v-else-if="debug.data.available">
          <el-descriptions :column="1" border size="small" class="debug-meta">
            <el-descriptions-item :label="t('logDetail.debug_meta_bytes')">
              {{ debug.data.bytes.toLocaleString() }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('logDetail.debug_meta_truncated')">
              {{ debug.data.truncated ? t('logDetail.yes') : t('logDetail.no') }}
            </el-descriptions-item>
          </el-descriptions>
          <pre class="debug-body"><code>{{ debug.pretty }}</code></pre>
          <p v-if="debug.parseError" class="hint">{{ t('logDetail.debugParseError') }}</p>
          <el-button size="small" @click="loadDebug" :disabled="debug.loading" class="debug-reload">
            {{ t('logDetail.debugReload') }}
          </el-button>
        </template>
        <p v-else class="hint">{{ t('logDetail.debugNone') }}</p>
      </section>

      <!-- Token 明细：cache 段只在该次请求非零时显示 -->
      <section>
        <h3>{{ t('logDetail.sectionTokens') }}</h3>
        <el-descriptions :column="1" border>
          <el-descriptions-item :label="t('logDetail.field.total')">{{ log.total_tokens.toLocaleString() }}</el-descriptions-item>
          <el-descriptions-item :label="t('logDetail.field.input')">{{ log.prompt_tokens.toLocaleString() }}</el-descriptions-item>
          <el-descriptions-item :label="t('logDetail.field.output')">{{ log.completion_tokens.toLocaleString() }}</el-descriptions-item>
          <el-descriptions-item v-if="log.cache_read_tokens > 0" :label="t('logDetail.field.cacheRead')">
            {{ log.cache_read_tokens.toLocaleString() }}
            <span class="hint">{{ t('logDetail.cacheReadHint') }}</span>
          </el-descriptions-item>
          <el-descriptions-item v-if="log.cache_creation_tokens > 0" :label="t('logDetail.field.cacheWrite')">
            {{ log.cache_creation_tokens.toLocaleString() }}
            <span class="hint">{{ t('logDetail.cacheWriteHint') }}</span>
          </el-descriptions-item>
          <el-descriptions-item v-if="log.reasoning_tokens > 0" :label="t('logDetail.field.reasoning')">
            {{ log.reasoning_tokens.toLocaleString() }}
            <span class="hint">{{ t('logDetail.reasoningHint') }}</span>
          </el-descriptions-item>
        </el-descriptions>
      </section>
    </template>
  </el-card>
</template>

<script setup>
import { computed, onMounted, reactive, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { i18n } from '../i18n'
import { getLog, getLogDebug } from '../api'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const log = ref(null)
const loading = ref(false)
const ok = ref(false)

async function load() {
  loading.value = true
  try {
    log.value = await getLog(route.params.id)
    ok.value = log.value && log.value.status_code >= 200 && log.value.status_code < 300
  } finally {
    loading.value = false
  }
}

// Captured upstream body. Held as { data, requested, error }:
//   - `data` null = not yet fetched (button visible).
//   - `data.available: false` = fetched, nothing to show.
//   - `data.available: true` = fetched, render the body.
const debug = reactive({
  data: null,
  loading: false,
  error: null,
  pretty: '',
  parseError: false,
})

async function loadDebug() {
  debug.loading = true
  debug.error = null
  try {
    const payload = await getLogDebug(route.params.id)
    debug.data = payload
    debug.pretty = payload.available ? formatBody(payload.body) : ''
  } catch (e) {
    debug.error = String(e)
  } finally {
    debug.loading = false
  }
}

// Pretty-print JSON when we can. The upstream body can be anything — a relay
// station may answer with an HTML error page, a half-streamed SSE chunk, or
// just `text/plain`, so a parse failure is the normal case for some of those.
// Fall back to showing the raw string so the admin always sees what arrived.
function formatBody(raw) {
  try {
    const obj = JSON.parse(raw)
    debug.parseError = false
    return JSON.stringify(obj, null, 2)
  } catch {
    debug.parseError = true
    return raw
  }
}

function formatTime(ts) {
  return new Date(ts * 1000).toLocaleString()
}

// Breakdown for the "转发链路" header. Skipped hops are routing decisions,
// not real attempts — show them in a separate bucket so the headline number
// matches what actually went over the network.
const attemptSummary = computed(() => {
  const atts = log.value?.attempts ?? []
  const skipped = atts.filter((a) => a.skipped).length
  const failed = log.value?.failed_count ?? 0
  const parts = [t('logDetail.attemptTotal', { n: atts.length })]
  if (skipped > 0) parts.push(t('logDetail.attemptSkipped', { n: skipped }))
  if (failed > 0) parts.push(t('logDetail.attemptFailed', { n: failed }))
  return parts.join(i18n.global.locale.value === 'en-US' ? ', ' : '，')
})

function attemptRowClass({ row }) {
  return row.skipped ? 'is-skipped' : ''
}

function back() {
  router.push('/logs')
}

onMounted(load)
</script>

<style scoped>
.header {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 16px;
}
.title {
  color: #606266;
  font-size: 13px;
}
.status-bar {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 12px;
}
.latency {
  font-variant-numeric: tabular-nums;
}
.err {
  color: #f56c6c;
}
/* Browser UAs run long. Let it wrap instead of clipping — this is the page
   where the full string is the point (the list page truncates instead). */
.ua-full {
  font-size: 12px;
  color: #606266;
  word-break: break-all;
}
/* Fix the label column so the long "请求模型 → 转发模型" label doesn't
   stretch it and leave a canyon of whitespace before each value. */
:deep(.el-descriptions__label) {
  width: 168px;
}
.note {
  margin: 6px 0 0;
}
section {
  margin-top: 20px;
}
h3 {
  margin: 0 0 8px;
  font-size: 14px;
  color: #303133;
}
/* Skipped attempts (circuit-breaker open) are deliberately de-emphasized —
   no HTTP call was made, so the row is bookkeeping rather than a real
   outcome. Dim the text so the eye skips over them while still showing
   that the relay considered them. */
:deep(.el-table .is-skipped td) {
  color: #c0c4cc;
  background-color: #fafafa;
}
.muted {
  color: #c0c4cc;
}

/* Captured upstream body. The body can be very long (up to the capture cap
   is 256 KB) — a fixed max-height with scroll is the right call; expanding
   to fit would push the token usage section off-screen. JSON is
   monospaced, so the eye reads it column-wise. */
.debug-meta {
  margin: 8px 0;
}
.debug-body {
  background-color: #1e1e1e;
  color: #d4d4d4;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 12px;
  line-height: 1.5;
  padding: 12px;
  border-radius: 4px;
  max-height: 480px;
  overflow: auto;
  white-space: pre;
  margin: 0;
}
.debug-reload {
  margin-top: 8px;
}
</style>