<template>
  <el-card>
    <div class="toolbar">
      <span>模型别名：把客户端传入的模型名（如 gpt-4o）重写为上游实际模型名，再进行渠道路由</span>
      <el-button type="primary" @click="openCreate">添加别名</el-button>
    </div>
    <el-table :data="mappings" v-loading="loading">
      <el-table-column prop="id" label="ID" width="60" />
      <el-table-column prop="alias" label="客户端调用名 (alias)" min-width="200" />
      <el-table-column label="→" width="60" align="center">
        <template #default>→</template>
      </el-table-column>
      <el-table-column prop="target_model" label="上游实际模型" min-width="200" />
      <el-table-column label="创建时间" width="180">
        <template #default="{ row }">
          {{ new Date(row.created_at * 1000).toLocaleString() }}
        </template>
      </el-table-column>
      <el-table-column label="操作" width="100">
        <template #default="{ row }">
          <el-popconfirm title="确认删除该别名？" @confirm="remove(row.id)">
            <template #reference>
              <el-button size="small" type="danger">删除</el-button>
            </template>
          </el-popconfirm>
        </template>
      </el-table-column>
    </el-table>

    <el-dialog v-model="dialogVisible" title="添加别名" width="460px" @closed="resetForm">
      <el-form :model="form" label-width="160px">
        <el-form-item label="客户端调用名">
          <el-input v-model="form.alias" placeholder="如 gpt-4o / claude-3-5-sonnet" />
        </el-form-item>
        <el-form-item label="上游实际模型">
          <el-input v-model="form.target_model" placeholder="如 kimi-k3 / glm-5.3" />
        </el-form-item>
        <el-form-item>
          <span class="hint">单层重写（a → b 不会再追到 c）。新建后立即对下一次请求生效。</span>
        </el-form-item>
      </el-form>
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
import { listMappings, createMapping, deleteMapping } from '../api'

const mappings = ref([])
const loading = ref(false)
const saving = ref(false)
const dialogVisible = ref(false)
const form = ref({ alias: '', target_model: '' })

async function load() {
  loading.value = true
  try {
    mappings.value = await listMappings()
  } finally {
    loading.value = false
  }
}

function openCreate() {
  form.value = { alias: '', target_model: '' }
  dialogVisible.value = true
}

async function submit() {
  if (!form.value.alias.trim() || !form.value.target_model.trim()) {
    ElMessage.warning('请填写两端模型名')
    return
  }
  saving.value = true
  try {
    try {
      await createMapping({
        alias: form.value.alias.trim(),
        target_model: form.value.target_model.trim(),
      })
    } catch (e) {
      // 409 = alias already exists; surface a clearer one
      if (e?.response?.status === 409) {
        ElMessage.error(`别名 "${form.value.alias}" 已存在`)
        return
      }
      throw e
    }
    ElMessage.success('已添加')
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
  form.value = { alias: '', target_model: '' }
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
}
</style>