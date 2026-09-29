<template>
  <el-card>
    <div class="toolbar">
      <span>外部渠道：配置上游 API 地址、密钥与支持的模型</span>
      <el-button type="primary" @click="openDialog()">添加渠道</el-button>
    </div>
    <el-table :data="channels" v-loading="loading">
      <el-table-column prop="id" label="ID" width="60" />
      <el-table-column prop="name" label="名称" width="160" />
      <el-table-column prop="base_url" label="Base URL" min-width="220" show-overflow-tooltip />
      <el-table-column prop="models" label="模型" min-width="200" show-overflow-tooltip />
      <el-table-column label="状态" width="90">
        <template #default="{ row }">
          <el-tag :type="row.enabled ? 'success' : 'danger'">
            {{ row.enabled ? '启用' : '禁用' }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column label="操作" width="160">
        <template #default="{ row }">
          <el-button size="small" @click="openDialog(row)">编辑</el-button>
          <el-popconfirm title="确认删除该渠道？" @confirm="remove(row.id)">
            <template #reference>
              <el-button size="small" type="danger">删除</el-button>
            </template>
          </el-popconfirm>
        </template>
      </el-table-column>
    </el-table>

    <el-dialog v-model="dialogVisible" :title="editing ? '编辑渠道' : '添加渠道'" width="560px">
      <el-form label-width="90px">
        <el-form-item label="名称">
          <el-input v-model="form.name" placeholder="如 openai-main" />
        </el-form-item>
        <el-form-item label="Base URL">
          <el-input v-model="form.base_url" placeholder="https://api.openai.com（不带 /v1）" />
        </el-form-item>
        <el-form-item label="API Key">
          <el-input v-model="form.api_key" placeholder="上游渠道密钥" show-password />
        </el-form-item>
        <el-form-item label="模型">
          <el-input v-model="form.models" type="textarea" :rows="3"
            placeholder="逗号分隔，如 gpt-4o,gpt-4o-mini,claude-3-5-sonnet" />
        </el-form-item>
        <el-form-item label="启用">
          <el-switch v-model="form.enabled" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dialogVisible = false">取消</el-button>
        <el-button type="primary" @click="save">保存</el-button>
      </template>
    </el-dialog>
  </el-card>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { listChannels, createChannel, updateChannel, deleteChannel } from '../api'

const channels = ref([])
const loading = ref(false)
const dialogVisible = ref(false)
const editing = ref(null)
const form = ref({ name: '', base_url: '', api_key: '', models: '', enabled: true })

async function load() {
  loading.value = true
  try {
    channels.value = await listChannels()
  } finally {
    loading.value = false
  }
}

function openDialog(row) {
  editing.value = row || null
  form.value = row
    ? { name: row.name, base_url: row.base_url, api_key: row.api_key, models: row.models, enabled: !!row.enabled }
    : { name: '', base_url: '', api_key: '', models: '', enabled: true }
  dialogVisible.value = true
}

async function save() {
  if (!form.value.name || !form.value.base_url || !form.value.api_key) {
    ElMessage.warning('名称、Base URL、API Key 不能为空')
    return
  }
  if (editing.value) {
    await updateChannel(editing.value.id, form.value)
  } else {
    await createChannel(form.value)
  }
  dialogVisible.value = false
  await load()
}

async function remove(id) {
  await deleteChannel(id)
  await load()
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
