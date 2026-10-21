<template>
  <el-card v-loading="loading">
    <div class="header">
      <el-button text @click="back">{{ t('logDetail.back') }}</el-button>
      <span class="title">{{ t('logDetail.title', { id: log?.id }) }}</span>
    </div>

    <template v-if="log">
      <!-- 一眼扫到的状态条 -->
      <div class="status-bar">
        <el-tag v-if="log.pending" type="info" size="large">
          <i class="el-icon-loading" style="margin-right: 4px;" />
          {{ t('logs.pending') }}
        </el-tag>
        <el-tag v-else :type="ok ? 'success' : 'danger'" size="large">{{ log.status_code }}</el-tag>
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
        <!-- Client hung up before the stream's terminator. Says why the
             token section below is empty; deliberately not styled as an
             error — the hop succeeded, the client just left early. -->
        <el-tooltip v-if="log.client_aborted" placement="top" :show-after="200" :content="t('logs.abortedTip')">
          <el-tag size="small" type="info" effect="plain">{{ t('logs.aborted') }}</el-tag>
        </el-tooltip>
      </div>

      <el-alert v-if="log && !log.pending && log.error" type="error" :title="log.error" :closable="false" show-icon />

      <!-- 基本信息 -->
      <section>
        <h3>{{ t('logDetail.sectionRequest') }}</h3>
        <el-descriptions :column="1" border>
          <el-descriptions-item :label="t('logDetail.field.token')">{{ log.token_name }}</el-descriptions-item>
          <el-descriptions-item :label="t('logDetail.field.protocol')">{{ log.protocol || '—' }}</el-descriptions-item>
          <el-descriptions-item :label="t('logDetail.field.clientIp')">{{ log.client_ip || '—' }}</el-descriptions-item>
          <el-descriptions-item :label="t('logDetail.field.time')">{{ formatTime(log.created_at) }}</el-descriptions-item>
          <!-- The value is the arrow, so the label only has to name the
               field: "请求模型 → 转发模型" said the same thing twice and
               was the one label too long to fit the column. -->
          <el-descriptions-item :label="t('logDetail.field.modelRoute')">
            {{ log.request_model || '—' }} → {{ log.upstream_model || log.request_model || '—' }}
          </el-descriptions-item>
          <!-- The two correlation IDs. Shown only when they exist: a client
               that sends no x-request-id leaves the first row out, and the
               gateway's is what `x-litrouter-request-id` echoed back. -->
          <el-descriptions-item v-if="log.client_request_id" :label="t('logDetail.field.clientRequestId')">
            <span class="mono">{{ log.client_request_id }}</span>
          </el-descriptions-item>
          <el-descriptions-item v-if="log.request_id" :label="t('logDetail.field.gatewayRequestId')">
            <span class="mono">{{ log.request_id }}</span>
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
              <el-tag v-else-if="row.status_code === 0" type="info" size="small">
                <i class="el-icon-loading" style="margin-right: 4px;" />
                {{ t('logDetail.inProgress') }}
              </el-tag>
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
          <!-- The upstream's own IDs for the hop. The whole point of the
               column: a provider is asked about these, not about a status
               code, when the reply it sent doesn't match the request. -->
          <el-table-column :label="t('logDetail.col.upstreamRefs')" min-width="130">
            <template #default="{ row }">
              <span v-if="row.upstream_request_id" class="mono">{{ row.upstream_request_id }}</span>
              <div v-if="row.upstream_cf_ray" class="mono sub">{{ row.upstream_cf_ray }}</div>
              <div v-if="row.upstream_generation_id" class="mono sub">{{ row.upstream_generation_id }}</div>
              <span
                v-if="!row.upstream_request_id && !row.upstream_cf_ray && !row.upstream_generation_id"
                class="hint"
              >—</span>
            </template>
          </el-table-column>
        </el-table>
        <p class="hint note">
          {{ t('logDetail.latencyNote') }}
        </p>
        <p class="hint note">
          {{ anyUpstreamRefs ? t('logDetail.upstreamRefsNote') : t('logDetail.idsNone') }}
        </p>
      </section>

      <!-- Token 明细：cache 段只在该次请求非零时显示 -->
      <section>
        <h3>{{ t('logDetail.sectionTokens') }}</h3>
        <el-descriptions :column="1" border>
          <el-descriptions-item :label="t('logDetail.field.total')">{{ log.total_tokens.toLocaleString() }}</el-descriptions-item>
          <el-descriptions-item :label="t('logDetail.field.input')">
            {{ log.prompt_tokens.toLocaleString() }}
            <span class="hint">{{ t('logDetail.inputHint') }}</span>
          </el-descriptions-item>
          <el-descriptions-item :label="t('logDetail.field.output')">
            {{ log.completion_tokens.toLocaleString() }}
            <span class="hint">{{ t('logDetail.outputHint') }}</span>
          </el-descriptions-item>
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
          <!-- TTFT: Time to First Token, only meaningful for streaming requests -->
          <el-descriptions-item v-if="log.stream && log.ttft_ms > 0" :label="t('logDetail.field.ttft')">
            {{ log.ttft_ms.toLocaleString() }} ms
            <span class="hint">{{ t('logDetail.ttftHint') }}</span>
          </el-descriptions-item>
        </el-descriptions>
      </section>
    </template>
  </el-card>
</template>

<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { i18n } from '../i18n'
import { getLog, openLogStream } from '../api'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const log = ref(null)
const loading = ref(false)
const ok = ref(false)
let stream = null

async function load(silent = false) {
  if (!silent) loading.value = true
  try {
    log.value = await getLog(route.params.id)
    ok.value = log.value?.pending ? null : (log.value && log.value.status_code >= 200 && log.value.status_code < 300)
  } finally {
    if (!silent) loading.value = false
  }
}

// A relay request emits a growing series of pending events (one hop goes in
// flight, one settles) and one final event. Replay the chain live: while the
// request is in flight, only the list-level fields + attempts change, so merge
// them into what `getLog` returned; once it settles, re-read the full row so
// error / protocol-conversion / token breakdown land too (the event only
// carries the list subset).
function applyEvent(ev) {
  if (!log.value || String(ev.id) !== String(route.params.id)) return
  log.value.pending = ev.pending
  log.value.status_code = ev.status_code
  log.value.latency_ms = ev.latency_ms
  log.value.total_tokens = ev.total_tokens
  log.value.failed_count = ev.failed_count
  if (ev.upstream_model) log.value.upstream_model = ev.upstream_model
  if (ev.channel_name) log.value.channel_name = ev.channel_name
  log.value.client_aborted = ev.client_aborted
  if (ev.attempts) log.value.attempts = ev.attempts.map((a) => ({ ...a }))
  ok.value = ev.pending
    ? null
    : ev.status_code >= 200 && ev.status_code < 300
  if (!ev.pending) load(true)
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

// Whether any hop carried upstream IDs at all. When none did, the note under
// the chain says so rather than repeating "quote these to the provider"
// advice about a column full of dashes.
const anyUpstreamRefs = computed(() =>
  (log.value?.attempts ?? []).some(
    (a) => a.upstream_request_id || a.upstream_cf_ray || a.upstream_generation_id
  )
)

function attemptRowClass({ row }) {
  return row.skipped ? 'is-skipped' : ''
}
function back() {
  router.push('/logs')
}

onMounted(async () => {
  await load()
  // Follow the relay chain while the request is still in flight, and pick up
  // the settled row as soon as the final event arrives.
  stream = openLogStream(applyEvent)
})

onUnmounted(() => {
  stream?.close()
  stream = null
})
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
/* Request IDs are meant to be copy-pasted verbatim, so no ellipsis on the
   result of a wide click — except in the sub-lines, where the extra CF ray
   and generation ID are supporting detail rather than the headline value. */
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12px;
  word-break: break-all;
}
.mono.sub {
  color: #909399;
}
/* The label column. Both tables on this page share one width, so their
   values line up down the page — the width is fixed rather than fitted to
   each table's longest label, which is what made the two tables disagree.
   `nowrap` is the other half: Element Plus renders the label as an
   inline-block span, and a label that wraps is a label that reads as two
   things. The labels are short enough to fit (the longest is
   "x-litrouter-request-id"), so nothing has to wrap. */
:deep(.el-descriptions__label) {
  width: 148px;
  white-space: nowrap;
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
</style>