<template>
  <el-card>
    <div class="toolbar">
      <span>调用日志</span>
      <el-button @click="load">刷新</el-button>
    </div>
    <el-table :data="logs" v-loading="loading">
      <el-table-column prop="id" label="ID" width="80" />
      <el-table-column prop="token_name" label="令牌" width="160" />
      <el-table-column prop="model" label="模型" width="220" />
      <el-table-column prop="channel_name" label="渠道" width="160" />
      <el-table-column label="状态码" width="90">
        <template #default="{ row }">
          <el-tag :type="row.status_code >= 200 && row.status_code < 300 ? 'success' : 'danger'">
            {{ row.status_code }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column label="时间" min-width="180">
        <template #default="{ row }">
          {{ new Date(row.created_at * 1000).toLocaleString() }}
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
import { listLogs } from '../api'

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
</style>
