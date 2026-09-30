<template>
  <el-card>
    <div class="toolbar">
      <span>模型路由：客户端按此模型名发起请求，网关按列表顺序依次转发到指定渠道/模型，前面的失败时自动回退到后面的</span>
      <el-button type="primary" @click="openCreate">添加路由</el-button>
    </div>
    <el-table :data="mappings" v-loading="loading">
      <el-table-column prop="alias" label="客户端模型名" min-width="160" />
      <el-table-column label="→" width="50" align="center">
        <template #default>→</template>
      </el-table-column>
      <el-table-column label="转发目标（按顺序）" min-width="360">
        <template #default="{ row }">
          <el-tag
            v-for="(t, i) in row.targets"
            :key="i"
            class="target-tag"
            :type="i === 0 ? 'primary' : 'info'"
          >
            {{ i + 1 }}. {{ formatTarget(t) }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column label="创建时间" width="180">
        <template #default="{ row }">
          {{ new Date(row.created_at * 1000).toLocaleString() }}
        </template>
      </el-table-column>
      <el-table-column label="操作" width="140">
        <template #default="{ row }">
          <el-button size="small" @click="openEdit(row)">编辑</el-button>
          <el-popconfirm title="确认删除该路由？" @confirm="remove(row.id)">
            <template #reference>
              <el-button size="small" type="danger">删除</el-button>
            </template>
          </el-popconfirm>
        </template>
      </el-table-column>
    </el-table>

    <el-dialog v-model="dialogVisible" :title="editingId ? '编辑路由' : '添加路由'" width="720px" @closed="resetForm">
      <el-form :model="form" label-position="top">
        <el-form-item label="客户端模型名">
          <el-input v-model="form.alias" placeholder="客户端请求里填的模型名，如 my-model" />
        </el-form-item>
      </el-form>
      <div class="target-section">
        <div class="target-section-title">上游模型列表</div>
        <div v-for="(t, i) in form.targets" :key="i" class="target-row">
          <div class="target-row-main">
            <span class="target-index">{{ i + 1 }}.</span>
            <el-select
              v-model="t.channel"
              filterable
              clearable
              placeholder="任意渠道"
              class="channel-select"
            >
              <el-option v-for="ch in channelModels" :key="ch.name" :label="ch.name" :value="ch.name" />
            </el-select>
            <el-select
              v-model="t.model"
              filterable
              allow-create
              default-first-option
              placeholder="选择或输入上游模型名"
              class="model-select"
            >
              <el-option v-if="t.channel" label="全部模型（该渠道下所有模型，按顺序尝试）" value="*" />
              <template v-if="t.channel">
                <el-option-group :label="`渠道：${t.channel}`">
                  <el-option
                    v-for="m in channelModelNames(t.channel)"
                    :key="m"
                    :label="m"
                    :value="m"
                  />
                </el-option-group>
              </template>
              <template v-else>
                <el-option-group v-for="ch in channelModels" :key="ch.name" :label="`渠道：${ch.name}`">
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
              删除
            </el-button>
          </div>
        </div>
        <el-button size="small" class="add-target" @click="addTarget">+ 添加目标</el-button>
        <div class="hint">
          请求按列表顺序尝试：第 1 个失败或无可用渠道时回退到第 2 个，依此类推。
          选定渠道即锁定转发到该渠道；模型选「全部模型」表示按该渠道的模型列表逐个尝试；
          渠道留空表示任意支持该模型的渠道。保存后立即对下一次请求生效。
        </div>
      </div>
      <template #footer>
        <el-button @click="dialogVisible = false">取消</el-button>
        <el-button type="primary" :loading="saving" @click="submit">保存</el-button>
      </template>
    </el-dialog>
  </el-card>

  <el-card v-if="isAdmin" class="breaker-card">
    <el-collapse v-model="breakerExpanded">
      <el-collapse-item name="breaker">
        <template #title>
          <div class="breaker-title">
            <span>熔断器状态</span>
            <span class="hint">
              {{ openCount }} 个 (渠道, 模型) 熔断中
            </span>
          </div>
        </template>
        <div class="breaker-actions">
          <el-button size="small" @click="refreshBreaker">刷新</el-button>
          <el-button size="small" type="danger" plain @click="onResetBreaker">一键复位</el-button>
        </div>
        <div class="hint breaker-desc">
          熔断中的上游组合会被路由自动跳过，不会消耗请求配额也不会出现在 attempt 链里。
          任意一次失败（5xx / 4xx / 408 / 429 / 网络错误）即熔断 {{ baseDelaySecs }}s；
          探测再次失败，退避每次翻倍（×2），最多到 {{ maxDelaySecs }}s。
          到点由后台任务独立发探测验证恢复，恢复前不会主动重试用户请求。
          后台任务每 {{ probeIntervalSecs }}s 检查一次所有到期组合。
          恢复正常后该组合会从此表移除。
        </div>
        <el-table
          :data="snapshot"
          v-loading="breakerLoading"
          size="small"
          empty-text="所有上游组合目前都正常"
        >
          <el-table-column prop="channel" label="渠道" width="160" show-overflow-tooltip />
          <el-table-column prop="target_model" label="模型" min-width="180" show-overflow-tooltip />
          <el-table-column label="下次探测" width="100">
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
import { listMappings, createMapping, updateMapping, deleteMapping, listChannelModels, me, getBreakerConfig } from '../api'
import { breaker, loadBreakerSnapshot, resetAllBreakers } from '../breaker'

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
const isAdmin = ref(false)
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

function formatTarget(t) {
  const model = t.model === '*' ? '全部模型' : t.model
  return t.channel ? `${t.channel}/${model}` : model
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
    ElMessage.warning('请填写客户端模型名')
    return
  }
  if (targets.length === 0) {
    ElMessage.warning('请至少填写一个转发目标')
    return
  }
  if (targets.some((t) => t.model === '*' && !t.channel)) {
    ElMessage.warning('「全部模型」需要先选定渠道')
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
        ElMessage.error(`模型名 "${form.value.alias}" 已存在`)
        return
      }
      throw e
    }
    ElMessage.success(editingId.value ? '已保存' : '已添加')
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
  try {
    isAdmin.value = !!(await me()).is_admin
  } catch (_) {
    // interceptor handles the redirect on 401
  }
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
