<template>
  <el-card>
    <div class="toolbar">
      <span>{{ t('tokens.description') }}</span>
      <el-button type="primary" @click="openCreate">{{ t('tokens.create') }}</el-button>
    </div>
    <el-table :data="tokens" v-loading="loading">
      <el-table-column prop="name" :label="t('tokens.col.name')" width="160" />
      <el-table-column v-if="isAdmin" :label="t('tokens.col.owner')" width="120">
        <template #default="{ row }">
          <span v-if="row.owner">{{ row.owner }}</span>
          <span v-else class="hint">{{ t('tokens.unassigned') }}</span>
        </template>
      </el-table-column>
      <el-table-column :label="t('tokens.col.key')" min-width="280">
        <template #default="{ row }">
          <span class="mono">{{ row.key }}</span>
          <el-button size="small" text @click="copyKey(row.key)">{{ t('tokens.copy') }}</el-button>
        </template>
      </el-table-column>
      <el-table-column :label="t('tokens.col.status')" width="80">
        <template #default="{ row }">
          <el-switch :model-value="!!row.enabled"
            @change="(v) => save(row, { enabled: v })" />
        </template>
      </el-table-column>
      <el-table-column :label="t('tokens.col.rpm')" width="110">
        <template #default="{ row }">
          <span v-if="row.rpm_limit > 0">{{ row.rpm_limit }} {{ t('common.minutes') }}</span>
          <span v-else class="hint">{{ t('common.unlimited') }}</span>
        </template>
      </el-table-column>
      <el-table-column :label="t('tokens.col.dailyQuota')" width="130">
        <template #default="{ row }">
          <span v-if="row.daily_token_limit > 0">{{ fmtNum(row.daily_token_limit) }} {{ t('common.days') }}</span>
          <span v-else class="hint">{{ t('common.unlimited') }}</span>
        </template>
      </el-table-column>
      <el-table-column :label="t('tokens.col.lastUsed')" width="170">
        <template #default="{ row }">
          {{ row.accessed_at ? new Date(row.accessed_at * 1000).toLocaleString() : '-' }}
        </template>
      </el-table-column>
      <el-table-column :label="t('tokens.col.actions')" width="160">
        <template #default="{ row }">
          <el-button size="small" @click="openEdit(row)">{{ t('tokens.quota') }}</el-button>
          <el-popconfirm :title="t('tokens.deleteConfirm')" @confirm="remove(row.id)">
            <template #reference>
              <el-button size="small" type="danger">{{ t('common.delete') }}</el-button>
            </template>
          </el-popconfirm>
        </template>
      </el-table-column>
    </el-table>

    <el-dialog v-model="dialogVisible"
      :title="editing ? t('tokens.editQuota') : t('tokens.create')"
      width="460px"
      @closed="resetForm">
      <el-form :model="form" label-width="120px">
        <el-form-item :label="t('tokens.col.name')">
          <el-input v-model="form.name" :disabled="editing" :placeholder="t('tokens.namePlaceholder')" />
        </el-form-item>
        <el-form-item v-if="!editing && isAdmin" :label="t('tokens.ownerLabel')">
          <el-select v-model="form.user_id" :placeholder="t('tokens.ownerPlaceholder')" clearable style="width: 220px">
            <el-option v-for="u in users" :key="u.id"
              :label="u.username + (u.is_admin ? t('tokens.adminSuffix') : '')"
              :value="u.id" />
          </el-select>
        </el-form-item>
        <el-form-item :label="t('channels.form.enabled')">
          <el-switch v-model="form.enabled" />
        </el-form-item>
        <el-form-item :label="t('tokens.col.rpm')">
          <el-input-number v-model="form.rpm_limit" :min="0" :step="10" style="width: 180px" />
          <span class="hint">{{ t('tokens.rpmHint') }}</span>
        </el-form-item>
        <el-form-item :label="t('tokens.col.dailyQuota')">
          <el-input-number v-model="form.daily_token_limit" :min="0" :step="10000" style="width: 180px" />
          <span class="hint">{{ t('tokens.dailyHint') }}</span>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dialogVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" :loading="saving" @click="submit">{{ editing ? t('common.save') : t('common.create') }}</el-button>
      </template>
    </el-dialog>
  </el-card>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { useI18n } from 'vue-i18n'
import { createToken, deleteToken, listTokens, listUsers, updateToken } from '../api'
import { loadSession, session } from '../session'
import { fmtNum } from '../format'

const { t } = useI18n()

const tokens = ref([])
const users = ref([])
const loading = ref(false)
const saving = ref(false)
const dialogVisible = ref(false)
const editing = ref(null)
const isAdmin = computed(() => session.isAdmin)

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
    ElMessage.warning(t('tokens.nameRequired'))
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
      ElMessage.success(t('common.saved'))
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
      ElMessage.success(t('tokens.created'))
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
  ElMessage.success(t('tokens.copied'))
}

function resetForm() {
  form.value = emptyForm()
  editing.value = null
}

onMounted(async () => {
  await loadSession()
  await load()
})
</script>

<style scoped>
.mono {
  font-family: monospace;
  font-size: 12px;
}
.hint {
  margin-left: 8px;
}
</style>