<template>
  <el-card>
    <div class="toolbar">
      <span>调用日志</span>
      <div class="filters">
        <el-radio-group v-model="range" @change="reload">
          <el-radio-button :value="1">一小时</el-radio-button>
          <el-radio-button :value="24">当日</el-radio-button>
          <el-radio-button :value="168">一周</el-radio-button>
          <el-radio-button :value="0">全部</el-radio-button>
        </el-radio-group>
        <el-button @click="load">刷新</el-button>
      </div>
    </div>
    <el-table :data="logs" v-loading="loading" @row-click="open">
      <el-table-column label="请求时间" width="165" show-overflow-tooltip>
        <template #default="{ row }">
          {{ new Date(row.created_at * 1000).toLocaleString() }}
        </template>
      </el-table-column>
      <el-table-column label="来源" width="290">
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
      <el-table-column prop="token_name" label="令牌" width="120" show-overflow-tooltip />
      <el-table-column prop="request_model" label="请求模型" min-width="150" show-overflow-tooltip />
      <el-table-column label="转发模型" min-width="200" show-overflow-tooltip>
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
                  前 {{ row.failed_attempts.length }} 次转发失败，已自动切换渠道
                </div>
                <div v-for="(f, i) in row.failed_attempts" :key="i" class="fail-tip-row">
                  {{ f.upstream_model }}（{{ f.channel_name || '无渠道' }}）— {{ f.error || f.status_code }}
                </div>
              </div>
            </template>
            <el-tag type="warning" size="small" effect="plain" class="fail-badge">
              失败 {{ row.failed_attempts.length }} 次
            </el-tag>
          </el-tooltip>
        </template>
      </el-table-column>
      <el-table-column label="状态码" width="90">
        <template #default="{ row }">
          <el-tag :type="row.status_code >= 200 && row.status_code < 300 ? 'success' : 'danger'">
            {{ row.status_code }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column label="Tokens" width="110" prop="total_tokens">
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
        <span class="debug-label">调试日志</span>
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
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { listLogs, me } from '../api'
import { debugLogging, loadDebugLogging, toggleDebugLogging } from '../debug'

const router = useRouter()
const logs = ref([])
const total = ref(0)
const loading = ref(false)
const page = ref(1)
const size = ref(20)
// Window in hours; 0 = all time. Default 1h to match the backend default.
const range = ref(1)
// The debug switch is admin-only server-side; regular users don't see it.
const isAdmin = ref(false)

async function onDebugToggle(v) {
  try {
    await toggleDebugLogging(v)
  } catch (_) {
    // Reverted in the shared state; the interceptor surfaces the 403.
  }
}

// Switching the window invalidates the current page number — page 4 of the
// old window is meaningless in the new one.
function reload() {
  page.value = 1
  load()
}

async function load() {
  loading.value = true
  try {
    const data = await listLogs(page.value, size.value, range.value)
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
  try {
    isAdmin.value = !!(await me()).is_admin
  } catch (_) {
    // interceptor handles the redirect on 401
  }
  if (isAdmin.value) loadDebugLogging()
  load()
})
</script>

<style scoped>
.toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 12px;
  color: #606266;
  font-size: 13px;
}
.filters {
  display: flex;
  align-items: center;
  gap: 8px;
}
.hint {
  color: #909399;
  font-size: 12px;
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
.num {
  font-variant-numeric: tabular-nums;
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