<template>
  <el-card>
    <div class="toolbar">
      <span>{{ t('breakerHistory.title') }}</span>
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
          :label="eventLabel(v, f.key)"
          :value="v"
        />
      </el-select>
      <el-button size="small" :disabled="!hasFilters" @click="resetFilters">
        {{ t('breakerHistory.filter.reset') }}
      </el-button>
    </div>
    <el-table :data="events" v-loading="loading" :empty-text="t('breakerHistory.empty')">
      <el-table-column :label="t('breakerHistory.col.time')" width="165" show-overflow-tooltip>
        <template #default="{ row }">
          {{ new Date(row.created_at * 1000).toLocaleString() }}
        </template>
      </el-table-column>
      <el-table-column :label="t('breakerHistory.col.event')" width="120">
        <template #default="{ row }">
          <el-tag :type="eventType(row.event)" size="small" effect="plain">
            {{ eventText(row.event) }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column
        :label="t('breakerHistory.col.channel')"
        prop="channel_name"
        width="160"
        show-overflow-tooltip
      />
      <el-table-column
        :label="t('breakerHistory.col.model')"
        prop="target_model"
        min-width="180"
        show-overflow-tooltip
      />
      <el-table-column
        :label="t('breakerHistory.col.reason')"
        prop="reason"
        min-width="220"
        show-overflow-tooltip
      />
      <el-table-column :label="t('breakerHistory.col.backoff')" width="100">
        <template #default="{ row }">
          <span v-if="row.backoff_secs">{{ row.backoff_secs }}s</span>
          <span v-else class="hint">—</span>
        </template>
      </el-table-column>
    </el-table>
    <div class="pager-row">
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
import { useI18n } from 'vue-i18n'
import { listBreakerHistory, listBreakerHistoryFilterOptions } from '../api'
import { loadSession, session } from '../session'

const { t } = useI18n()

const events = ref([])
const total = ref(0)
const loading = ref(false)
const page = ref(1)
const size = ref(20)
// Same window semantics as Logs.vue: hours, 0 = all time. Default 1h.
const range = ref(1)

// Three dropdowns: channel + model come from the server; event is a closed
// set rendered verbatim from i18n so adding a new event kind only needs
// editing the i18n files + the closed-set table on the server.
const FILTER_FIELDS = [
  { key: 'channel', label: 'breakerHistory.filter.channel', cls: 'f-channel' },
  { key: 'model', label: 'breakerHistory.filter.model', cls: 'f-model' },
  { key: 'event', label: 'breakerHistory.filter.event', cls: 'f-event' },
]
const filters = reactive({ channel: '', model: '', event: '' })
// Closed set, mirrors the server's BREAKER_EVENT_VALUES. Declared before
// `options` so the `ref({ ..., event: EVENT_VALUES })` initializer can
// read it — `const` doesn't hoist like `var`.
const EVENT_VALUES = ['tripped', 're-tripped', 'recovered', 'reset_all', 'reset_key']
// Server returns facet values per filter; event is closed so its list is
// supplied by `EVENT_VALUES` rather than the server response.
const options = ref({ channel: [], model: [], event: EVENT_VALUES })
const optionsLoading = ref(false)
const hasFilters = computed(() => FILTER_FIELDS.some((f) => filters[f.key] !== ''))

// Element Plus tag colors — danger for failure, success for recovery, info
// for admin actions. Same intent as the snapshot panel's "open" tag.
function eventType(name) {
  if (name === 'tripped') return 'danger'
  if (name === 're-tripped') return 'warning'
  if (name === 'recovered') return 'success'
  return 'info'
}

function eventText(name) {
  const key = {
    tripped: 'tripped',
    're-tripped': 'retripped',
    recovered: 'recovered',
    reset_all: 'resetAll',
    reset_key: 'resetKey',
  }[name]
  return key ? t(`breakerHistory.event.${key}`) : name
}

// Event dropdown options show translated labels but submit the raw enum
// the server expects. The other two dropdowns already carry their own
// values verbatim.
function eventLabel(v, key) {
  if (key !== 'event') return String(v)
  return eventText(v)
}

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
    const data = await listBreakerHistoryFilterOptions(range.value, filterParams())
    // Server returns { channel, model, event: [...server-side enum...] }.
    // event is overridden with our i18n-fed list so the dropdown labels
    // match the column tag translations.
    options.value = {
      channel: data.channel || [],
      model: data.model || [],
      event: EVENT_VALUES,
    }
  } catch (_) {
    // Keep the previous options — empty lists would hide any selectable
    // value. The axios interceptor surfaces the error.
  } finally {
    optionsLoading.value = false
  }
}

const isAdmin = computed(() => session.isAdmin)

function reload() {
  page.value = 1
  load()
}

async function load() {
  loading.value = true
  try {
    const data = await listBreakerHistory(page.value, size.value, range.value, filterParams())
    events.value = data.events
    total.value = data.total
    // Same pager-stale-page handling as Logs.vue: if a deletion left us
    // past the last page, snap back and reload.
    if (page.value > 1 && events.value.length === 0) {
      page.value = Math.max(1, Math.ceil(total.value / size.value))
      return load()
    }
  } finally {
    loading.value = false
  }
}

onMounted(async () => {
  await loadSession()
  if (isAdmin.value) {
    load()
    loadOptions()
  }
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
.f-channel,
.f-model {
  width: 190px;
}
.f-event {
  width: 150px;
}
.hint {
  color: #909399;
}
.pager-row {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
  margin-top: 12px;
}
</style>