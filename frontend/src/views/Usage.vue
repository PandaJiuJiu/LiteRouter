<template>
  <el-card v-loading="loading">
    <div class="toolbar">
      <span>{{ t('usage.description') }}</span>
      <el-radio-group v-model="range" @change="load">
        <el-radio-button :value="1">{{ t('usage.range.today') }}</el-radio-button>
        <el-radio-button :value="7">{{ t('usage.range.days7') }}</el-radio-button>
        <el-radio-button :value="30">{{ t('usage.range.days30') }}</el-radio-button>
      </el-radio-group>
    </div>

    <div class="stats-grid">
      <div class="stat-card">
        <div class="stat-label">{{ t('usage.totalRequests') }}</div>
        <div class="stat-value">{{ fmtNum(totes.requests) }}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">{{ t('usage.stat.promptTokens') }}</div>
        <div class="stat-value">{{ fmtNum(totes.prompt_tokens) }}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">{{ t('usage.stat.completionTokens') }}</div>
        <div class="stat-value">{{ fmtNum(totes.completion_tokens) }}</div>
      </div>
      <div class="stat-card highlight">
        <div class="stat-label">{{ t('usage.stat.totalTokens') }}</div>
        <div class="stat-value">{{ fmtNum(totes.total_tokens) }}</div>
      </div>
    </div>

    <div class="chart-layout">
      <div class="chart-panel">
        <div class="chart-head">
          <span class="chart-title">{{ t(`usage.tab.${activeTab}`) }}</span>
          <el-radio-group v-model="metric" size="small">
            <el-radio-button value="requests">{{ t('usage.col.requests') }}</el-radio-button>
            <el-radio-button value="prompt_tokens">{{ t('usage.col.promptTokens') }}</el-radio-button>
            <el-radio-button value="completion_tokens">{{ t('usage.col.completionTokens') }}</el-radio-button>
            <el-radio-button value="total_tokens">{{ t('usage.col.totalTokens') }}</el-radio-button>
          </el-radio-group>
        </div>
        <usage-bar-chart
          :rows="chartRows"
          :value-key="metric"
          :label-key="chartLabelKey"
          :is-day="activeTab === 'day'"
          :metric-label="metricLabel"
        />
      </div>

      <div class="table-panel">
        <el-tabs v-model="activeTab">
      <el-tab-pane :label="t('usage.tab.token')" name="token">
        <el-table :data="by_token" stripe>
          <el-table-column prop="key" :label="t('usage.col.internalToken')" min-width="160" />
          <usage-token-columns />
        </el-table>
      </el-tab-pane>
      <el-tab-pane :label="t('usage.tab.model')" name="model">
        <el-table :data="by_model" stripe>
          <el-table-column prop="key" :label="t('usage.col.model')" min-width="200" />
          <usage-token-columns />
        </el-table>
      </el-tab-pane>
      <el-tab-pane :label="t('usage.tab.channel')" name="channel">
        <el-table :data="by_channel" stripe>
          <el-table-column prop="key" :label="t('usage.col.channel')" min-width="160" />
          <usage-token-columns />
        </el-table>
      </el-tab-pane>
      <el-tab-pane :label="t('usage.tab.day')" name="day">
        <el-table :data="by_day" stripe>
          <el-table-column :label="t('usage.col.date')" min-width="160">
            <template #default="{ row }">{{ fmtDate(row.day) }}</template>
          </el-table-column>
          <usage-token-columns />
        </el-table>
      </el-tab-pane>
        </el-tabs>
      </div>
    </div>
  </el-card>
</template>

<script setup>
import { onMounted, ref, computed } from 'vue'
import { useI18n } from 'vue-i18n'
import UsageTokenColumns from './UsageTokenColumns.vue'
import UsageBarChart from './UsageBarChart.vue'
import { fetchUsage } from '../api'
import { fmtDate, fmtNum } from '../format'

const { t } = useI18n()

const range = ref(7)
const loading = ref(false)
const activeTab = ref('token')
// Which numeric column the chart plots. Defaults to total tokens (the headline
// number); switching it re-renders the same rows against another field.
const metric = ref('total_tokens')
const totes = ref({ requests: 0, prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 })
const by_token = ref([])
const by_model = ref([])
const by_channel = ref([])
const by_day = ref([])

// The bar chart mirrors whichever table is on screen. `key` is the label field
// for the three categorical tabs; the day tab keys off its timestamp instead.
const chartRows = computed(
  () =>
    ({
      token: by_token.value,
      model: by_model.value,
      channel: by_channel.value,
      day: by_day.value,
    })[activeTab.value] || [],
)
const chartLabelKey = computed(() => (activeTab.value === 'day' ? 'day' : 'key'))
const metricLabel = computed(
  () =>
    ({
      requests: t('usage.col.requests'),
      prompt_tokens: t('usage.col.promptTokens'),
      completion_tokens: t('usage.col.completionTokens'),
      total_tokens: t('usage.col.totalTokens'),
    })[metric.value] || '',
)

async function load() {
  loading.value = true
  try {
    const data = await fetchUsage(range.value)
    totes.value = data.totals || totes.value
    by_token.value = data.by_token || []
    by_model.value = data.by_model || []
    // A request where every candidate failed is logged with a blank
    // `channel_name` (the backend refuses to blame the last channel tried),
    // so those requests group into an empty-key bucket. It is real traffic,
    // but it is not a channel — showing it here would be a nameless row of
    // requests with zero tokens. It stays counted in `totals` and stays
    // visible per-token / per-model / per-day; drop it from this tab only.
    // Caveat: a channel saved with an empty name is filtered out too — the
    // backend doesn't validate `name`, and such a row would be unusable here
    // regardless.
    by_channel.value = (data.by_channel || []).filter((r) => r.key)
    by_day.value = data.by_day || []
  } finally {
    loading.value = false
  }
}

onMounted(load)
</script>

<style scoped>
.toolbar {
  margin-bottom: 16px;
}
.stats-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
  gap: 12px;
  margin-bottom: 20px;
}
.stat-card {
  border: 1px solid #e4e7ed;
  border-radius: 8px;
  padding: 16px 18px;
  background: #fafafa;
}
.stat-card.highlight {
  background: #ecf5ff;
  border-color: #409eff;
}
.stat-label {
  font-size: 12px;
  color: #909399;
  margin-bottom: 6px;
}
.stat-value {
  font-size: 22px;
  font-weight: 600;
  color: #303133;
}
.stat-card.highlight .stat-value {
  color: #409eff;
}
.chart-layout {
  display: flex;
  gap: 20px;
  align-items: stretch;
  margin-bottom: 20px;
}
.chart-panel {
  flex: 1 1 44%;
  min-width: 0;
  padding: 12px 16px 8px;
  border: 1px solid #e4e7ed;
  border-radius: 8px;
}
.table-panel {
  flex: 1 1 56%;
  min-width: 0;
  padding: 12px 16px 8px;
  border: 1px solid #e4e7ed;
  border-radius: 8px;
}
@media (max-width: 1100px) {
  .chart-layout {
    flex-direction: column;
  }
  .chart-panel,
  .table-panel {
    width: 100%;
  }
}
.chart-head {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 6px;
}
.chart-title {
  font-size: 14px;
  font-weight: 600;
  color: #303133;
}
.hint {
  margin-top: 16px;
}
</style>