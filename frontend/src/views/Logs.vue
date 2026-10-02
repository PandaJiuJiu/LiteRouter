<template>
  <el-card>
    <div class="toolbar">
      <span>{{ t('logs.title') }}</span>
      <div class="filters">
        <el-radio-group v-model="range" @change="applyFilters">
          <el-radio-button :value="1">{{ t('logs.range.hour') }}</el-radio-button>
          <el-radio-button :value="24">{{ t('logs.range.day') }}</el-radio-button>
          <el-radio-button :value="168">{{ t('logs.range.week') }}</el-radio-button>
          <el-radio-button :value="0">{{ t('logs.range.all') }}</el-radio-button>
        </el-radio-group>
        <el-button @click="refresh">{{ t('common.refresh') }}</el-button>
      </div>
    </div>
    <div class="filter-row">
      <el-select
        v-for="f in FILTER_FIELDS"
        :key="f.key"
        v-model="filters[f.key]"
        size="small"
        filterable
        clearable
        :loading="optionsLoading"
        :placeholder="t(f.label)"
        :class="f.cls"
        @change="applyFilters"
      >
        <el-option
          v-for="v in options[f.key] || []"
          :key="String(v)"
          :label="String(v)"
          :value="v"
        />
      </el-select>
      <el-button size="small" :disabled="!hasFilters" @click="resetFilters">
        {{ t('logs.filter.reset') }}
      </el-button>
    </div>
    <el-table :data="logs" v-loading="loading" @row-click="open">
      <el-table-column :label="t('logs.col.time')" width="165" show-overflow-tooltip>
        <template #default="{ row }">
          {{ new Date(row.created_at * 1000).toLocaleString() }}
        </template>
      </el-table-column>
      <el-table-column :label="t('logs.col.source')" width="290">
        <template #default="{ row }">
          <el-tooltip
            v-if="row.client_ip || row.user_agent"
            placement="top"
            :show-after="200"
            :content="[row.client_ip, row.user_agent].filter(Boolean).join(' · ')"
          >
            <span class="src">
              <span v-if="row.client_ip" class="ip">{{ row.client_ip }}</span>
              <span v-if="row.client_ip && row.user_agent"> · </span>
              <span v-if="row.user_agent" class="ua">{{ shortUa(row.user_agent) }}</span>
            </span>
          </el-tooltip>
          <span v-else class="hint">—</span>
        </template>
      </el-table-column>
      <el-table-column prop="token_name" :label="t('logs.col.token')" width="120" show-overflow-tooltip />
      <el-table-column prop="request_model" :label="t('logs.col.requestModel')" min-width="150" show-overflow-tooltip />
      <el-table-column :label="t('logs.col.upstreamModel')" min-width="200" show-overflow-tooltip>
        <template #default="{ row }">
          <span>{{ row.upstream_model || row.request_model }}</span>
          <span class="hint">（{{ row.channel_name }}）</span>
          <el-tooltip
            v-if="row.failed_attempts?.length"
            placement="top"
            :show-after="200"
          >
            <template #content>
              <div class="fail-tip">
                <div class="fail-tip-title">
                  {{ t('logs.failTipTitle', { count: row.failed_attempts.length }) }}
                </div>
                <div v-for="(f, i) in row.failed_attempts" :key="i" class="fail-tip-row">
                  {{ f.upstream_model }}（{{ f.channel_name || t('logs.noChannel') }}）— {{ f.error || f.status_code }}
                </div>
              </div>
            </template>
            <el-tag type="warning" size="small" effect="plain" class="fail-badge">
              {{ t('logs.failBadge', { count: row.failed_attempts.length }) }}
            </el-tag>
          </el-tooltip>
        </template>
      </el-table-column>
      <el-table-column :label="t('logs.col.statusCode')" width="90">
        <template #default="{ row }">
          <el-tag :type="row.status_code >= 200 && row.status_code < 300 ? 'success' : 'danger'">
            {{ row.status_code }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column :label="t('logs.col.tokens')" width="110" prop="total_tokens">
        <template #default="{ row }">
          <span v-if="row.total_tokens > 0" class="num">{{ fmt(row.total_tokens) }}</span>
          <span v-else class="hint">—</span>
        </template>
      </el-table-column>
      <el-table-column label="" width="50">
        <template #default>
          <span class="chev">›</span>
        </template>
      </el-table-column>
    </el-table>
    <div class="pager-row">
      <div v-if="isAdmin" class="debug-cell">
        <span class="debug-label">{{ t('logs.debugLogging') }}</span>
        <el-switch
          :model-value="debugLogging.enabled"
          size="small"
          :loading="debugLogging.loading"
          @change="onDebugToggle"
        />
      </div>
      <el-pagination
        layout="total, sizes, prev, pager, next"
        :total="total"
        :page-size="size"
        :current-page="page"
        :page-sizes="[20, 50, 100, 200]"
        @current-change="(p) => { page = p; load() }"
        @size-change="(s) => { size = s; page = 1; load() }"
      />
    </div>
  </el-card>
</template>

<script setup>
import { computed, onMounted, reactive, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { listLogFilterOptions, listLogs } from '../api'
import { loadSession, session } from '../session'
import { debugLogging, loadDebugLogging, toggleDebugLogging } from '../debug'

const { t } = useI18n()
const router = useRouter()
const logs = ref([])
const total = ref(0)
const loading = ref(false)
const page = ref(1)
const size = ref(20)
// Window in hours; 0 = all time. Default 1h to match the backend default.
const range = ref(1)
// The five dropdowns. `key` is both the query-param name and the field the
// option list arrives under, so the row renders itself; `upstream_model` is the
// one snake_case key, which is why `filters` uses it verbatim.
const FILTER_FIELDS = [
  { key: 'ip', label: 'logs.filter.ip', cls: 'f-ip' },
  { key: 'token', label: 'logs.filter.token', cls: 'f-token' },
  { key: 'model', label: 'logs.filter.model', cls: 'f-model' },
  { key: 'upstream_model', label: 'logs.filter.upstreamModel', cls: 'f-model' },
  { key: 'status', label: 'logs.filter.status', cls: 'f-status' },
]
// Selection state, applied on change — there's nothing to type, so a dropdown
// pick *is* the intent. `''` means "no filter".
const filters = reactive({ ip: '', token: '', model: '', upstream_model: '', status: '' })
// Values each dropdown may offer, refetched whenever the filters or the window
// change: the server computes every dropdown without its own filter, so
// narrowing by token narrows the IP list too (facet rule) — an option that had
// no matching row left is never offered.
const options = ref({})
const optionsLoading = ref(false)
const hasFilters = computed(() => FILTER_FIELDS.some((f) => filters[f.key] !== ''))

/** The current selection as query params. Unset boxes are dropped rather than
 *  sent empty — `status=` would deserialize to nothing server-side and 400 the
 *  whole list. */
function filterParams() {
  const params = {}
  for (const { key } of FILTER_FIELDS) {
    const v = filters[key]
    if (v !== '' && v !== null && v !== undefined) params[key] = v
  }
  return params
}

function applyFilters() {
  reload()
  loadOptions()
}

// Plain refresh keeps the current page — unlike `applyFilters`, it isn't a
// change of result set. The dropdowns still refresh: new traffic in the window
// means new values to offer.
function refresh() {
  load()
  loadOptions()
}

function resetFilters() {
  for (const { key } of FILTER_FIELDS) filters[key] = ''
  applyFilters()
}

async function loadOptions() {
  optionsLoading.value = true
  try {
    options.value = await listLogFilterOptions(range.value, filterParams())
  } catch (_) {
    // Keep the previous options: an empty list would leave every dropdown blank
    // and hide the fact that anything was ever selectable. The interceptor
    // surfaces the 403/500.
  } finally {
    optionsLoading.value = false
  }
}

// The debug switch is admin-only server-side; regular users don't see it.
const isAdmin = computed(() => session.isAdmin)

async function onDebugToggle(v) {
  try {
    await toggleDebugLogging(v)
  } catch (_) {
    // Reverted in the shared state; the interceptor surfaces the 403.
  }
}

// Switching the window invalidates the current page number — page 4 of the
// old window is meaningless in the new one. The filter dropdowns go with it:
// their options are scoped to the window, so they have to be refetched too.
function reload() {
  page.value = 1
  load()
}

async function load() {
  loading.value = true
  try {
    const data = await listLogs(page.value, size.value, range.value, filterParams())
    logs.value = data.logs
    total.value = data.total
    // A stale page (e.g. after deletions) can leave us past the last page.
    if (page.value > 1 && logs.value.length === 0) {
      page.value = Math.max(1, Math.ceil(total.value / size.value))
      return load()
    }
  } finally {
    loading.value = false
  }
}

function fmt(n) {
  return Number(n || 0).toLocaleString()
}

// User-Agents are long and mostly redundant past the product token — the
// parenthetical at the end of a browser UA is a spec, not a signal. 24 chars
// covers the SDKs verbatim (claude-cli/2.1.284, OpenAI/Python 1.92.2,
// curl/8.4.0, python-httpx/0.27.0) and cuts browsers down to "Mozilla/5.0
// (Macintosh…", which is enough to tell a browser apart from an SDK. The
// tooltip on the cell still shows the whole string.
const UA_MAX = 24

function shortUa(ua) {
  if (!ua || ua.length <= UA_MAX) return ua
  return ua.slice(0, UA_MAX - 1) + '…'
}

function open(row) {
  router.push(`/logs/${row.id}`)
}

onMounted(async () => {
  await loadSession()
  if (isAdmin.value) {
    loadDebugLogging()
  }
  load()
  // The dropdowns are scoped to the window, so they move with it.
  loadOptions()
})
</script>

<style scoped>
.filters {
  display: flex;
  align-items: center;
  gap: 8px;
}
.filter-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  margin-bottom: 10px;
}
/* Fixed-ish widths rather than flex:1 — the five boxes should read as one
   toolbar, and equal stretch makes the 3-digit status box as wide as the IP
   one. */
.f-ip,
.f-token {
  width: 170px;
}
.f-model {
  width: 190px;
}
.f-status {
  width: 110px;
}
.ip {
  font-size: 12px;
  color: #303133;
}
.ua {
  font-size: 11px;
  color: #909399;
}
/* Without this the cell wraps to two lines once the content exceeds the
   column width; the ellipsis + tooltip path is easier to read. */
.src {
  display: inline-block;
  max-width: 100%;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  vertical-align: bottom;
}
.fail-badge {
  margin-left: 6px;
  cursor: default;
}
.fail-tip-title {
  margin-bottom: 4px;
  font-weight: 600;
}
.fail-tip-row {
  font-size: 12px;
  opacity: 0.9;
}
/* Switch sits immediately before el-pagination's own "Total N" slot — the
   pagination component renders its summary inline, so the two share a row.
   `space-between` pins the debug toggle to the left edge and pushes the
   pagination to the right. */
.pager-row {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 12px;
  flex-wrap: wrap;
  justify-content: space-between;
}
.debug-cell {
  display: flex;
  align-items: center;
  gap: 6px;
}
.debug-label {
  font-size: 12px;
  color: #909399;
}
:deep(.el-table__row) {
  cursor: pointer;
}
.chev {
  color: #c0c4cc;
  font-size: 18px;
  line-height: 1;
}
</style>