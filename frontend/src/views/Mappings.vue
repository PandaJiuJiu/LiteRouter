<template>
  <el-card>
    <div class="toolbar">
      <span>模型路由：客户端按此模型名发起请求，网关按列表顺序依次转发到指定渠道/模型，前面的失败时自动回退到后面的</span>
      <el-button type="primary" @click="openCreate">添加路由</el-button>
    </div>
    <el-table :data="mappings" v-loading="loading">
      <el-table-column prop="id" label="ID" width="60" />
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
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { listMappings, createMapping, updateMapping, deleteMapping, listChannelModels } from '../api'

const mappings = ref([])
const channelModels = ref([])
const loading = ref(false)
const saving = ref(false)
const dialogVisible = ref(false)
const editingId = ref(null)
const form = ref({ alias: '', targets: [] })

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
</style>
