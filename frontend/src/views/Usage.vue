<template>
  <el-card v-loading="loading">
    <div class="toolbar">
      <span>Token 用量与请求统计</span>
      <el-radio-group v-model="range" @change="load">
        <el-radio-button :value="1">今日</el-radio-button>
        <el-radio-button :value="7">7 天</el-radio-button>
        <el-radio-button :value="30">30 天</el-radio-button>
      </el-radio-group>
    </div>

    <div class="stats-grid">
      <div class="stat-card">
        <div class="stat-label">总请求数</div>
        <div class="stat-value">{{ fmtNum(totes.requests) }}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Prompt tokens</div>
        <div class="stat-value">{{ fmtNum(totes.prompt_tokens) }}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Completion tokens</div>
        <div class="stat-value">{{ fmtNum(totes.completion_tokens) }}</div>
      </div>
      <div class="stat-card highlight">
        <div class="stat-label">Total tokens</div>
        <div class="stat-value">{{ fmtNum(totes.total_tokens) }}</div>
      </div>
    </div>

    <el-tabs v-model="activeTab">
      <el-tab-pane label="按 Token" name="token">
        <el-table :data="by_token" stripe>
          <el-table-column prop="key" label="内部 token" min-width="160" />
          <el-table-column prop="requests" label="请求数" width="120" sortable />
          <el-table-column prop="prompt_tokens" label="Prompt" width="140" sortable :formatter="fmt" />
          <el-table-column prop="completion_tokens" label="Completion" width="140" sortable :formatter="fmt" />
          <el-table-column prop="total_tokens" label="Total" width="140" sortable :formatter="fmt" />
        </el-table>
      </el-tab-pane>
      <el-tab-pane label="按 Model" name="model">
        <el-table :data="by_model" stripe>
          <el-table-column prop="key" label="模型" min-width="200" />
          <el-table-column prop="requests" label="请求数" width="120" sortable />
          <el-table-column prop="prompt_tokens" label="Prompt" width="140" sortable :formatter="fmt" />
          <el-table-column prop="completion_tokens" label="Completion" width="140" sortable :formatter="fmt" />
          <el-table-column prop="total_tokens" label="Total" width="140" sortable :formatter="fmt" />
        </el-table>
      </el-tab-pane>
      <el-tab-pane label="按 Channel" name="channel">
        <el-table :data="by_channel" stripe>
          <el-table-column prop="key" label="上游渠道" min-width="160" />
          <el-table-column prop="requests" label="请求数" width="120" sortable />
          <el-table-column prop="prompt_tokens" label="Prompt" width="140" sortable :formatter="fmt" />
          <el-table-column prop="completion_tokens" label="Completion" width="140" sortable :formatter="fmt" />
          <el-table-column prop="total_tokens" label="Total" width="140" sortable :formatter="fmt" />
        </el-table>
      </el-tab-pane>
      <el-tab-pane label="按日" name="day">
        <el-table :data="by_day" stripe>
          <el-table-column label="日期" min-width="160">
            <template #default="{ row }">{{ fmtDate(row.day) }}</template>
          </el-table-column>
          <el-table-column prop="requests" label="请求数" width="120" sortable />
          <el-table-column prop="prompt_tokens" label="Prompt" width="140" sortable :formatter="fmt" />
          <el-table-column prop="completion_tokens" label="Completion" width="140" sortable :formatter="fmt" />
          <el-table-column prop="total_tokens" label="Total" width="140" sortable :formatter="fmt" />
        </el-table>
      </el-tab-pane>
    </el-tabs>

    <p class="hint">注：流式（stream）响应不在统计范围内——上游的 usage 在最后一个 SSE chunk 里，需要特殊解析。后续增强。</p>
  </el-card>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { fetchUsage } from '../api'

const range = ref(7)
const loading = ref(false)
const activeTab = ref('token')
const totes = ref({ requests: 0, prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 })
const by_token = ref([])
const by_model = ref([])
const by_channel = ref([])
const by_day = ref([])

async function load() {
  loading.value = true
  try {
    const data = await fetchUsage(range.value)
    totes.value = data.totals || totes.value
    by_token.value = data.by_token || []
    by_model.value = data.by_model || []
    by_channel.value = data.by_channel || []
    by_day.value = data.by_day || []
  } finally {
    loading.value = false
  }
}

function fmtNum(n) {
  return Number(n || 0).toLocaleString()
}

function fmt(row, col, val) {
  return Number(val || 0).toLocaleString()
}

function fmtDate(unixDay) {
  // unixDay is start-of-day timestamp in seconds
  if (!unixDay) return ''
  const d = new Date(unixDay * 1000)
  const y = d.getFullYear()
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  return `${y}-${m}-${day}`
}

onMounted(load)
</script>

<style scoped>
.toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 16px;
  color: #606266;
  font-size: 13px;
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
.hint {
  color: #909399;
  font-size: 12px;
  margin-top: 16px;
}
</style>