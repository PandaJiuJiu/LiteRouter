<template>
  <el-card>
    <div class="toolbar">
      <span>外部渠道：配置上游 API 地址、密钥与支持的模型</span>
      <el-button type="primary" @click="openDialog()">添加渠道</el-button>
    </div>
    <el-table :data="channels" v-loading="loading">
      <el-table-column prop="id" label="ID" width="60" />
      <el-table-column prop="name" label="名称" width="160" />
      <el-table-column label="Base URL" min-width="240" show-overflow-tooltip>
        <template #default="{ row }">
          <div>OpenAI: {{ row.base_url || '-' }}</div>
          <div v-if="row.base_url_anthropic">Anthropic: {{ row.base_url_anthropic }}</div>
        </template>
      </el-table-column>
      <el-table-column prop="models" label="模型" min-width="200" show-overflow-tooltip>
        <template #default="{ row }">
          {{ row.models === '*' ? '全部模型（*）' : (row.models || '未配置（去"模型管理"页配置）') }}
        </template>
      </el-table-column>
      <el-table-column label="类型" width="90">
        <template #default="{ row }">
          <el-tag :type="row.kind === 'internal' ? 'info' : 'primary'">
            {{ row.kind === 'internal' ? '内部' : '外部' }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column label="状态" width="90">
        <template #default="{ row }">
          <el-tag :type="row.enabled ? 'success' : 'danger'">
            {{ row.enabled ? '启用' : '禁用' }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column label="操作" width="230">
        <template #default="{ row }">
          <el-button size="small" @click="openDialog(row)">编辑</el-button>
          <el-button size="small" type="primary" plain @click="manageModels(row)">模型管理</el-button>
          <el-popconfirm title="确认删除该渠道？" @confirm="remove(row.id)">
            <template #reference>
              <el-button size="small" type="danger">删除</el-button>
            </template>
          </el-popconfirm>
        </template>
      </el-table-column>
    </el-table>

    <el-dialog v-model="dialogVisible" :title="editing ? '编辑渠道' : '添加渠道'" width="560px">
      <el-form label-width="120px">
        <el-form-item label="名称">
          <el-input v-model="form.name" placeholder="渠道名称" />
        </el-form-item>
        <el-form-item label="OpenAI URL">
          <el-input v-model="form.base_url"
            placeholder="兼容 OpenAI 协议的地址（含完整路径，如 https://api.openai.com/v1）" />
        </el-form-item>
        <el-form-item label="Anthropic URL">
          <el-input v-model="form.base_url_anthropic"
            placeholder="兼容 Anthropic 协议的地址（含完整路径，如 https://api.anthropic.com/v1）" />
        </el-form-item>
        <el-form-item label="API Key">
          <el-input v-model="form.api_key" placeholder="上游渠道密钥" show-password />
        </el-form-item>
        <el-form-item label="类型">
          <el-radio-group v-model="form.kind">
            <el-radio value="external">外部（参与路由）</el-radio>
            <el-radio value="internal">内部（不参与路由）</el-radio>
          </el-radio-group>
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
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { listChannels, createChannel, updateChannel, deleteChannel } from '../api'

const channels = ref([])
const loading = ref(false)
const dialogVisible = ref(false)
const editing = ref(null)
const form = ref({ name: '', base_url: '', base_url_anthropic: '', api_key: '', models: '', enabled: true, kind: 'external' })

async function load() {
  loading.value = true
  try {
    channels.value = await listChannels()
  } finally {
    loading.value = false
  }
}

const router = useRouter()

function manageModels(row) {
  router.push(`/models?channel=${row.id}`)
}

function openDialog(row) {
  editing.value = row || null
  form.value = row
    ? { name: row.name, base_url: row.base_url, base_url_anthropic: row.base_url_anthropic || '', api_key: row.api_key, models: row.models, enabled: !!row.enabled, kind: row.kind || 'external' }
    : { name: '', base_url: '', base_url_anthropic: '', api_key: '', models: '', enabled: true, kind: 'external' }
  dialogVisible.value = true
}

async function save() {
  if (!form.value.name || !form.value.api_key) {
    ElMessage.warning('名称、API Key 不能为空')
    return
  }
  if (!form.value.base_url && !form.value.base_url_anthropic) {
    ElMessage.warning('OpenAI URL 和 Anthropic URL 至少填写一个')
    return
  }
  if (editing.value) {
    // models are managed on the models page — pass current value through
    await updateChannel(editing.value.id, { ...form.value, models: editing.value.models })
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
