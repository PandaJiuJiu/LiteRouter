<template>
  <el-card>
    <div class="toolbar">
      <span>{{ t('channels.description') }}</span>
      <el-button type="primary" @click="openDialog()">{{ t('channels.add') }}</el-button>
    </div>
    <el-table :data="channels" v-loading="loading">
      <el-table-column prop="name" :label="t('channels.col.name')" width="180">
        <template #default="{ row }">
          <a v-if="row.website" :href="row.website" target="_blank" rel="noopener"
            class="name-link">{{ row.name }}<el-icon class="ext-icon"><Link /></el-icon></a>
          <span v-else>{{ row.name }}</span>
          <el-tag v-if="row.proxy_effective" size="small" type="warning" effect="light" class="proxy-tag">
            <el-icon><Connection /></el-icon>
            <span class="proxy-text">{{ t('channels.proxyTag') }}</span>
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column :label="t('channels.col.baseUrl')" min-width="240" show-overflow-tooltip>
        <template #default="{ row }">
          <div>OpenAI: {{ row.base_url || '-' }}</div>
          <div v-if="row.base_url_anthropic">Anthropic: {{ row.base_url_anthropic }}</div>
        </template>
      </el-table-column>
      <el-table-column prop="models" :label="t('channels.col.models')" min-width="200" show-overflow-tooltip>
        <template #default="{ row }">
          {{ row.models === '*' ? t('channels.allModels') : (row.models || t('channels.notConfigured')) }}
        </template>
      </el-table-column>
      <el-table-column :label="t('channels.col.status')" width="90">
        <template #default="{ row }">
          <el-switch
            :model-value="!!row.enabled"
            @change="(val) => toggleEnabled(row, val)"
          />
        </template>
      </el-table-column>
      <el-table-column :label="t('channels.col.actions')" width="230">
        <template #default="{ row }">
          <el-button size="small" @click="openDialog(row)">{{ t('common.edit') }}</el-button>
          <el-button size="small" type="primary" plain @click="manageModels(row)">{{ t('channels.manageModels') }}</el-button>
          <el-popconfirm :title="t('channels.deleteConfirm')" @confirm="remove(row.id)">
            <template #reference>
              <el-button size="small" type="danger">{{ t('common.delete') }}</el-button>
            </template>
          </el-popconfirm>
        </template>
      </el-table-column>
    </el-table>

    <el-dialog v-model="dialogVisible" :title="editing ? t('channels.editChannel') : t('channels.addChannel')" width="560px">
      <el-form label-width="120px">
        <el-form-item :label="t('channels.form.name')">
          <el-input v-model="form.name" :placeholder="t('channels.form.namePlaceholder')" />
        </el-form-item>
        <el-form-item :label="t('channels.form.website')">
          <el-input v-model="form.website" :placeholder="t('channels.form.websitePlaceholder')" />
        </el-form-item>
        <el-form-item :label="t('channels.col.openaiUrl')">
          <el-input v-model="form.base_url"
            :placeholder="t('channels.form.openaiUrlPlaceholder')" />
        </el-form-item>
        <el-form-item :label="t('channels.col.anthropicUrl')">
          <el-input v-model="form.base_url_anthropic"
            :placeholder="t('channels.form.anthropicUrlPlaceholder')" />
        </el-form-item>
        <el-form-item :label="t('channels.col.apiKey')">
          <el-input v-model="form.api_key" :placeholder="t('channels.form.apiKeyPlaceholder')" show-password />
        </el-form-item>
        <el-form-item :label="t('channels.form.useProxy')">
          <el-switch v-model="form.use_proxy" />
          <!-- Per-channel toggle is an OR with the global switch, not a
               sub-switch of it: this channel goes through the proxy on its
               own, and the global switch can force other channels through it.
               Saying so here is the only place the relationship is visible. -->
          <div class="form-hint">{{ t('channels.form.useProxyHint') }}</div>
        </el-form-item>
        <el-form-item :label="t('channels.form.enabled')">
          <el-switch v-model="form.enabled" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dialogVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" @click="save">{{ t('common.save') }}</el-button>
      </template>
    </el-dialog>
  </el-card>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { useI18n } from 'vue-i18n'
import { Link, Connection } from '@element-plus/icons-vue'
import { listChannels, createChannel, updateChannel, deleteChannel } from '../api'

const { t } = useI18n()

const channels = ref([])
const loading = ref(false)
const dialogVisible = ref(false)
const editing = ref(null)
const form = ref({ name: '', website: '', base_url: '', base_url_anthropic: '', api_key: '', models: '', enabled: true, use_proxy: false })

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
    ? { name: row.name, website: row.website || '', base_url: row.base_url, base_url_anthropic: row.base_url_anthropic || '', api_key: row.api_key, models: row.models, enabled: !!row.enabled, use_proxy: !!row.use_proxy }
    : { name: '', website: '', base_url: '', base_url_anthropic: '', api_key: '', models: '', enabled: true, use_proxy: false }
  dialogVisible.value = true
}

async function save() {
  if (!form.value.name || !form.value.api_key) {
    ElMessage.warning(t('channels.nameKeyRequired'))
    return
  }
  if (!form.value.base_url && !form.value.base_url_anthropic) {
    ElMessage.warning(t('channels.urlRequired'))
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

async function toggleEnabled(row, val) {
  const prev = row.enabled
  row.enabled = val ? 1 : 0   // optimistic update — flip locally first
  try {
    await updateChannel(row.id, { ...row, enabled: val })
  } catch (e) {
    row.enabled = prev        // revert on failure
    const detail = e?.response?.data?.message || e.message || t('common.unknownError')
    ElMessage.error(t('channels.toggleFailed', { detail }))
  }
}

onMounted(load)
</script>

<style scoped>
.name-link {
  color: var(--el-color-primary);
  text-decoration: none;
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.name-link:hover {
  text-decoration: underline;
}
.ext-icon {
  font-size: 12px;
  opacity: 0.6;
}
.proxy-tag {
  margin-left: 8px;
  vertical-align: middle;
}
.proxy-text {
  margin-left: 2px;
}
.form-hint {
  font-size: 12px;
  color: var(--el-text-color-secondary);
  line-height: 1.5;
  margin-top: 2px;
}
</style>
