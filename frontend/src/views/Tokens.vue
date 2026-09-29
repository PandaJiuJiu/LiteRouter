<template>
  <el-card>
    <div class="toolbar">
      <span>内部令牌：内部服务使用这些 key 访问统一 API</span>
      <el-button type="primary" @click="create">创建令牌</el-button>
    </div>
    <el-table :data="tokens" v-loading="loading">
      <el-table-column prop="id" label="ID" width="60" />
      <el-table-column prop="name" label="名称" width="180" />
      <el-table-column label="Key" min-width="280">
        <template #default="{ row }">
          <span class="mono">{{ row.key }}</span>
          <el-button size="small" text @click="copyKey(row.key)">复制</el-button>
        </template>
      </el-table-column>
      <el-table-column label="状态" width="90">
        <template #default="{ row }">
          <el-switch :model-value="!!row.enabled"
            @change="(v) => toggle(row.id, v)" />
        </template>
      </el-table-column>
      <el-table-column label="最近使用" width="170">
        <template #default="{ row }">
          {{ row.accessed_at ? new Date(row.accessed_at * 1000).toLocaleString() : '-' }}
        </template>
      </el-table-column>
      <el-table-column label="操作" width="90">
        <template #default="{ row }">
          <el-popconfirm title="确认删除该令牌？" @confirm="remove(row.id)">
            <template #reference>
              <el-button size="small" type="danger">删除</el-button>
            </template>
          </el-popconfirm>
        </template>
      </el-table-column>
    </el-table>
  </el-card>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { listTokens, createToken, toggleToken, deleteToken } from '../api'

const tokens = ref([])
const loading = ref(false)

async function load() {
  loading.value = true
  try {
    tokens.value = await listTokens()
  } finally {
    loading.value = false
  }
}

async function create() {
  let name
  try {
    ({ value: name } = await ElMessageBox.prompt('输入令牌名称', '创建令牌'))
  } catch {
    return
  }
  if (!name) return
  await createToken({ name })
  await load()
}

async function toggle(id, enabled) {
  await toggleToken(id, enabled)
  await load()
}

async function remove(id) {
  await deleteToken(id)
  await load()
}

function copyKey(key) {
  navigator.clipboard.writeText(key)
  ElMessage.success('已复制')
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
.mono {
  font-family: monospace;
  font-size: 12px;
}
</style>
