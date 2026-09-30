<template>
  <el-card>
    <div class="toolbar">
      <span>调用日志</span>
      <el-button @click="load">刷新</el-button>
    </div>
    <el-table :data="logs" v-loading="loading" @row-click="open">
      <el-table-column label="请求时间" min-width="180">
        <template #default="{ row }">
          {{ new Date(row.created_at * 1000).toLocaleString() }}
        </template>
      </el-table-column>
      <el-table-column prop="token_name" label="令牌" width="160" />
      <el-table-column label="模型" min-width="280">
        <template #default="{ row }">
          <span v-if="row.request_model && row.request_model !== row.model">
            {{ row.request_model }}<span class="hint">→</span>{{ row.model }}
          </span>
          <span v-else>{{ row.model }}</span>
          <span class="hint">（{{ row.channel_name }}）</span>
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
    <el-pagination style="margin-top: 12px; justify-content: flex-end"
      layout="prev, pager, next" :page-size="size" :current-page="page"
      @current-change="(p) => { page = p; load() }" />
  </el-card>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { listLogs } from '../api'

const router = useRouter()
const logs = ref([])
const loading = ref(false)
const page = ref(1)
const size = 50

async function load() {
  loading.value = true
  try {
    logs.value = await listLogs(page.value, size)
  } finally {
    loading.value = false
  }
}

function fmt(n) {
  return Number(n || 0).toLocaleString()
}

function open(row) {
  router.push(`/logs/${row.id}`)
}

onMounted(load)
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
.hint {
  color: #909399;
  font-size: 12px;
}
.num {
  font-variant-numeric: tabular-nums;
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