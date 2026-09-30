<template>
  <el-card>
    <div class="toolbar">
      <span>内部令牌：内部服务使用这些 key 访问统一 API</span>
      <el-button type="primary" @click="openCreate">创建令牌</el-button>
    </div>
    <el-table :data="tokens" v-loading="loading">
      <el-table-column prop="name" label="名称" width="160" />
      <el-table-column v-if="isAdmin" label="归属" width="120">
        <template #default="{ row }">
          <span v-if="row.owner">{{ row.owner }}</span>
          <span v-else class="hint">未分配</span>
        </template>
      </el-table-column>
      <el-table-column label="Key" min-width="280">
        <template #default="{ row }">
          <span class="mono">{{ row.key }}</span>
          <el-button size="small" text @click="copyKey(row.key)">复制</el-button>
        </template>
      </el-table-column>
      <el-table-column label="状态" width="80">
        <template #default="{ row }">
          <el-switch :model-value="!!row.enabled"
            @change="(v) => save(row, { enabled: v })" />
        </template>
      </el-table-column>
      <el-table-column label="RPM 限制" width="110">
        <template #default="{ row }">
          <span v-if="row.rpm_limit > 0">{{ row.rpm_limit }} / 分钟</span>
          <span v-else class="hint">不限</span>
        </template>
      </el-table-column>
      <el-table-column label="日 Token 配额" width="130">
        <template #default="{ row }">
          <span v-if="row.daily_token_limit > 0">{{ fmt(row.daily_token_limit) }} / 天</span>
          <span v-else class="hint">不限</span>
        </template>
      </el-table-column>
      <el-table-column label="最近使用" width="170">
        <template #default="{ row }">
          {{ row.accessed_at ? new Date(row.accessed_at * 1000).toLocaleString() : '-' }}
        </template>
      </el-table-column>
      <el-table-column label="操作" width="160">
        <template #default="{ row }">
          <el-button size="small" @click="openEdit(row)">配额</el-button>
          <el-popconfirm title="确认删除该令牌？" @confirm="remove(row.id)">
            <template #reference>
              <el-button size="small" type="danger">删除</el-button>
            </template>
          </el-popconfirm>
        </template>
      </el-table-column>
    </el-table>

    <el-dialog v-model="dialogVisible"
      :title="editing ? '编辑配额' : '创建令牌'"
      width="460px"
      @closed="resetForm">
      <el-form :model="form" label-width="120px">
        <el-form-item label="名称">
          <el-input v-model="form.name" :disabled="editing" placeholder="内部服务名 / 用途" />
        </el-form-item>
        <el-form-item v-if="!editing && isAdmin" label="归属用户">
          <el-select v-model="form.user_id" placeholder="默认归属自己" clearable style="width: 220px">
            <el-option v-for="u in users" :key="u.id"
              :label="u.username + (u.is_admin ? ' (管理员)' : '')"
              :value="u.id" />
          </el-select>
        </el-form-item>
        <el-form-item label="启用">
          <el-switch v-model="form.enabled" />
        </el-form-item>
        <el-form-item label="RPM 限制">
          <el-input-number v-model="form.rpm_limit" :min="0" :step="10" style="width: 180px" />
          <span class="hint"> 次 / 分钟（0 = 不限）</span>
        </el-form-item>
        <el-form-item label="日 Token 配额">
          <el-input-number v-model="form.daily_token_limit" :min="0" :step="10000" style="width: 180px" />
          <span class="hint"> tokens / UTC 日（0 = 不限）</span>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dialogVisible = false">取消</el-button>
        <el-button type="primary" :loading="saving" @click="submit">{{ editing ? '保存' : '创建' }}</el-button>
      </template>
    </el-dialog>
  </el-card>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { createToken, deleteToken, listTokens, listUsers, me, updateToken } from '../api'

const tokens = ref([])
const users = ref([])
const loading = ref(false)
const saving = ref(false)
const dialogVisible = ref(false)
const editing = ref(null)
const isAdmin = ref(false)

const emptyForm = () => ({
  name: '',
  enabled: true,
  rpm_limit: 0,
  daily_token_limit: 0,
  user_id: null,
})
const form = ref(emptyForm())

async function load() {
  loading.value = true
  try {
    tokens.value = await listTokens()
    if (isAdmin.value) users.value = await listUsers()
  } finally {
    loading.value = false
  }
}

async function loadMe() {
  try {
    const u = await me()
    isAdmin.value = !!u.is_admin
  } catch (_) {}
}

function openCreate() {
  editing.value = null
  form.value = emptyForm()
  dialogVisible.value = true
}

function openEdit(row) {
  editing.value = row
  form.value = {
    name: row.name,
    enabled: !!row.enabled,
    rpm_limit: row.rpm_limit || 0,
    daily_token_limit: row.daily_token_limit || 0,
    user_id: row.user_id || null,
  }
  dialogVisible.value = true
}

async function submit() {
  if (!form.value.name.trim()) {
    ElMessage.warning('请输入名称')
    return
  }
  saving.value = true
  try {
    if (editing.value) {
      await updateToken(editing.value.id, {
        enabled: form.value.enabled,
        rpm_limit: Math.max(0, form.value.rpm_limit || 0),
        daily_token_limit: Math.max(0, form.value.daily_token_limit || 0),
      })
      ElMessage.success('已保存')
    } else {
      const payload = {
        name: form.value.name.trim(),
        enabled: form.value.enabled,
        rpm_limit: Math.max(0, form.value.rpm_limit || 0),
        daily_token_limit: Math.max(0, form.value.daily_token_limit || 0),
      }
      if (isAdmin.value && form.value.user_id) {
        payload.user_id = form.value.user_id
      }
      await createToken(payload)
      ElMessage.success('已创建')
    }
    dialogVisible.value = false
    await load()
  } finally {
    saving.value = false
  }
}

async function save(row, patch) {
  await updateToken(row.id, {
    enabled: patch.enabled ?? !!row.enabled,
    rpm_limit: row.rpm_limit || 0,
    daily_token_limit: row.daily_token_limit || 0,
  })
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

function fmt(n) {
  return Number(n || 0).toLocaleString()
}

function resetForm() {
  form.value = emptyForm()
  editing.value = null
}

onMounted(async () => {
  await loadMe()
  await load()
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
.mono {
  font-family: monospace;
  font-size: 12px;
}
.hint {
  color: #909399;
  font-size: 12px;
  margin-left: 8px;
}
</style>