<template>
  <el-card>
    <div class="toolbar">
      <span>{{ t('mappings.description') }}</span>
      <el-button type="primary" @click="openCreate">{{ t('mappings.add') }}</el-button>
    </div>
    <el-table :data="mappings" v-loading="loading">
      <el-table-column prop="alias" :label="t('mappings.col.alias')" min-width="160" />
      <el-table-column label="→" width="50" align="center">
        <template #default>→</template>
      </el-table-column>
      <el-table-column :label="t('mappings.col.targets')" min-width="360">
        <template #default="{ row }">
          <el-tag
            v-for="(tgt, i) in row.targets"
            :key="i"
            class="target-tag"
            :type="i === 0 ? 'primary' : 'info'"
          >
            {{ i + 1 }}. {{ formatTarget(tgt) }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column :label="t('mappings.col.created')" width="180">
        <template #default="{ row }">
          {{ new Date(row.created_at * 1000).toLocaleString() }}
        </template>
      </el-table-column>
      <el-table-column :label="t('mappings.col.actions')" width="140">
        <template #default="{ row }">
          <el-button size="small" @click="openEdit(row)">{{ t('common.edit') }}</el-button>
          <el-popconfirm :title="t('mappings.deleteConfirm')" @confirm="remove(row.id)">
            <template #reference>
              <el-button size="small" type="danger">{{ t('common.delete') }}</el-button>
            </template>
          </el-popconfirm>
        </template>
      </el-table-column>
    </el-table>

    <el-dialog v-model="dialogVisible" :title="editingId ? t('mappings.editRoute') : t('mappings.add')" width="720px" @closed="resetForm">
      <el-form :model="form" label-position="top">
        <el-form-item :label="t('mappings.aliasLabel')">
          <el-input v-model="form.alias" :placeholder="t('mappings.aliasPlaceholder')" />
        </el-form-item>
      </el-form>
      <div class="target-section">
        <div class="target-section-title">{{ t('mappings.targetSection') }}</div>
        <div v-for="(tgt, i) in form.targets" :key="i" class="target-row">
          <div class="target-row-main">
            <span class="target-index">{{ i + 1 }}.</span>
            <el-select
              v-model="tgt.channel"
              filterable
              clearable
              :placeholder="t('mappings.anyChannel')"
              class="channel-select"
            >
              <el-option v-for="ch in channelModels" :key="ch.name" :label="ch.name" :value="ch.name" />
            </el-select>
            <el-select
              v-model="tgt.model"
              filterable
              allow-create
              default-first-option
              :placeholder="t('mappings.modelPlaceholder')"
              class="model-select"
            >
              <el-option v-if="tgt.channel" :label="t('mappings.allModelsOption')" value="*" />
              <template v-if="tgt.channel">
                <el-option-group :label="t('mappings.channelGroup', { channel: tgt.channel })">
                  <el-option
                    v-for="m in channelModelNames(tgt.channel)"
                    :key="m"
                    :label="m"
                    :value="m"
                  />
                </el-option-group>
              </template>
              <template v-else>
                <el-option-group v-for="ch in channelModels" :key="ch.name" :label="t('mappings.channelGroup', { channel: ch.name })">
                  <el-option v-for="m in ch.models" :key="`${ch.name}/${m}`" :label="m" :value="m" />
                </el-option-group>
              </template>
            </el-select>
          </div>
          <div class="target-row-ops">
            <el-button class="arrow-btn" size="small" :disabled="i === 0" @click="move(i, -1)">↑</el-button>
            <el-button class="arrow-btn" size="small" :disabled="i === form.targets.length - 1" @click="move(i, 1)">↓</el-button>
            <el-button
              size="small"
              type="danger"
              :disabled="form.targets.length <= 1"
              @click="form.targets.splice(i, 1)"
            >
              {{ t('common.delete') }}
            </el-button>
          </div>
        </div>
        <el-button size="small" class="add-target" @click="addTarget">{{ t('mappings.addTarget') }}</el-button>
        <div class="hint">{{ t('mappings.help') }}</div>
      </div>
      <template #footer>
        <el-button @click="dialogVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" :loading="saving" @click="submit">{{ t('common.save') }}</el-button>
      </template>
    </el-dialog>
  </el-card>

  <el-card v-if="isAdmin" class="breaker-card">
    <el-collapse v-model="breakerExpanded">
      <el-collapse-item name="breaker">
        <template #title>
          <div class="breaker-title">
            <span>{{ t('mappings.breaker.title') }}</span>
            <span class="hint">
              {{ t('mappings.breaker.openCount', { count: openCount }) }}
            </span>
          </div>
        </template>
        <div class="breaker-actions">
          <el-button size="small" @click="refreshBreaker">{{ t('common.refresh') }}</el-button>
          <el-button size="small" type="danger" plain @click="onResetBreaker">{{ t('mappings.breaker.reset') }}</el-button>
        </div>
        <div class="hint breaker-desc">{{ t('mappings.breaker.desc', breakerParams) }}</div>
        <el-table
          :data="snapshot"
          v-loading="breakerLoading"
          size="small"
          :empty-text="t('mappings.breaker.allHealthy')"
        >
          <el-table-column prop="channel" :label="t('mappings.breaker.col.channel')" width="160" show-overflow-tooltip />
          <el-table-column prop="target_model" :label="t('mappings.breaker.col.model')" min-width="180" show-overflow-tooltip />
          <el-table-column :label="t('mappings.breaker.col.nextProbe')" width="100">
            <template #default="{ row }">
              <span v-if="row.cooldown_remaining_secs">{{ row.cooldown_remaining_secs }}s</span>
              <span v-else class="hint">—</span>
            </template>
          </el-table-column>
        </el-table>
      </el-collapse-item>
    </el-collapse>
  </el-card>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { useI18n } from 'vue-i18n'
import { translate } from '../i18n'
import { listMappings, createMapping, updateMapping, deleteMapping, listChannelModels, getBreakerConfig } from '../api'
import { loadSession, session } from '../session'
import { breaker, loadBreakerSnapshot, resetAllBreakers } from '../breaker'

const { t } = useI18n()

const mappings = ref([])
const channelModels = ref([])
const loading = ref(false)
const saving = ref(false)
const dialogVisible = ref(false)
const editingId = ref(null)
const form = ref({ alias: '', targets: [] })

// Circuit-breaker snapshot — admin only. Surfaced here because the breaker
// state is the per-(channel, model) health that *directly* determines which
// routing entries get skipped on the next request; admins editing mappings
// need to see it in the same view.
const isAdmin = computed(() => session.isAdmin)
// Default folded: most edits don't need the panel. Admin can expand it;
// the open/closed state isn't persisted across reloads on purpose — the
// snapshot itself is what's worth keeping an eye on, not the UI preference.
const breakerExpanded = ref([])
const breakerLoading = ref(false)
// Thresholds shown in the panel description — read from the server config
// so the docs in the UI match what's actually enforced.
const baseDelaySecs = ref(30)
const maxDelaySecs = ref(600)
const probeIntervalSecs = ref(30)
const snapshot = computed(() => breaker.snapshot)
const openCount = computed(
  () => snapshot.value.filter((r) => r.state === 'open').length,
)
// Breaker doc text interpolates the live thresholds, which come from the
// server so the description matches what's actually enforced.
const breakerParams = computed(() => ({
  base: baseDelaySecs.value,
  max: maxDelaySecs.value,
  probe: probeIntervalSecs.value,
}))

async function refreshBreaker() {
  breakerLoading.value = true
  try {
    await loadBreakerSnapshot()
  } finally {
    breakerLoading.value = false
  }
}
async function onResetBreaker() {
  await resetAllBreakers()
}

function newTarget() {
  return { channel: '', model: '' }
}

async function load() {
  loading.value = true
  try {
    const [m, ch] = await Promise.all([listMappings(), listChannelModels()])
    mappings.value = m
    channelModels.value = ch
  } finally {
    loading.value = false
  }
}

function channelModelNames(name) {
  const ch = channelModels.value.find((c) => c.name === name)
  return ch ? ch.models : []
}

function formatTarget(tgt) {
  const model = tgt.model === '*' ? translate('mappings.allModels') : tgt.model
  return tgt.channel ? `${tgt.channel}/${model}` : model
}

function openCreate() {
  editingId.value = null
  form.value = { alias: '', targets: [newTarget()] }
  dialogVisible.value = true
}

function openEdit(row) {
  editingId.value = row.id
  form.value = {
    alias: row.alias,
    targets: row.targets.map((t) => ({ channel: t.channel || '', model: t.model })),
  }
  dialogVisible.value = true
}

function addTarget() {
  form.value.targets.push(newTarget())
}

function move(i, delta) {
  const targets = form.value.targets
  const [item] = targets.splice(i, 1)
  targets.splice(i + delta, 0, item)
}

async function submit() {
  const targets = form.value.targets
    .map((t) => ({ channel: (t.channel || '').trim(), model: (t.model || '').trim() }))
    .filter((t) => t.model)
  if (!form.value.alias.trim()) {
    ElMessage.warning(t('mappings.aliasRequired'))
    return
  }
  if (targets.length === 0) {
    ElMessage.warning(t('mappings.needOneTarget'))
    return
  }
  if (targets.some((t) => t.model === '*' && !t.channel)) {
    ElMessage.warning(t('mappings.allModelsNeedChannel'))
    return
  }
  saving.value = true
  try {
    const payload = { alias: form.value.alias.trim(), targets }
    try {
      if (editingId.value) {
        await updateMapping(editingId.value, payload)
      } else {
        await createMapping(payload)
      }
    } catch (e) {
      // 409 = alias already exists; surface a clearer one
      if (e?.response?.status === 409) {
        ElMessage.error(t('mappings.aliasExists', { alias: form.value.alias }))
        return
      }
      throw e
    }
    ElMessage.success(editingId.value ? t('common.saved') : t('mappings.added'))
    dialogVisible.value = false
    await load()
  } finally {
    saving.value = false
  }
}

async function remove(id) {
  await deleteMapping(id)
  await load()
}

function resetForm() {
  editingId.value = null
  form.value = { alias: '', targets: [] }
}

onMounted(async () => {
  await loadSession()
  await load()
  if (isAdmin.value) {
    try {
      const cfg = await getBreakerConfig()
      baseDelaySecs.value = cfg.base_delay_secs ?? 30
      maxDelaySecs.value = cfg.max_delay_secs ?? 600
      probeIntervalSecs.value = cfg.probe_interval_secs ?? 30
    } catch (_) {
      // fall back to the defaults already in the refs
    }
    refreshBreaker()
  }
})
</script>

<style scoped>
.hint {
  line-height: 1.6;
  margin-top: 8px;
}
.target-tag {
  margin-right: 6px;
  margin-bottom: 4px;
}
.target-section {
  margin-bottom: 16px;
}
.target-section-title {
  font-size: 14px;
  color: #606266;
  margin-bottom: 10px;
}
.target-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
  padding: 8px;
  border: 1px solid #e4e7ed;
  border-radius: 4px;
  margin-bottom: 8px;
}
.target-row-main {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
}
.target-row-ops {
  display: flex;
  align-items: center;
  flex-shrink: 0;
}
/* ↑↓ 箭头按钮：缩窄外框、两按钮紧挨 */
.target-row-ops :deep(.arrow-btn) {
  padding: 5px 6px;
}
.target-row-ops :deep(.arrow-btn + .arrow-btn) {
  margin-left: 4px;
}
.target-row-ops :deep(.el-button + .el-button:not(.arrow-btn)) {
  margin-left: 12px;
}
.target-index {
  color: #909399;
  font-size: 13px;
  width: 18px;
  flex-shrink: 0;
}
.channel-select {
  width: 180px;
  flex-shrink: 0;
}
.model-select {
  flex: 1;
}
.add-target {
  margin-top: 4px;
}
/* Circuit breaker panel — sits below the mappings table because the
   breaker state is the per-(channel, model) health that directly
   determines which routing entries get skipped on the next request.
   Admins editing a mapping need to see the live state right here. */
.breaker-card {
  margin-top: 16px;
}
.breaker-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 8px;
}
.breaker-title {
  display: flex;
  gap: 12px;
  align-items: baseline;
}
.breaker-actions {
  display: flex;
  gap: 8px;
}
.breaker-desc {
  margin-bottom: 12px;
  line-height: 1.7;
}
</style>
