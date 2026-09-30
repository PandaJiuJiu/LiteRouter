<template>
  <el-card>
    <div class="toolbar">
      <span>系统用户：管理员可创建、修改、降级或删除账号</span>
      <el-button type="primary" @click="openCreate">新建用户</el-button>
    </div>

    <el-table :data="users" v-loading="loading">
      <el-table-column prop="username" label="用户名" width="200" />
      <el-table-column label="角色" width="140">
        <template #default="{ row }">
          <el-tag v-if="row.is_admin" type="success" size="small">管理员</el-tag>
          <el-tag v-else size="small">普通用户</el-tag>
        </template>
      </el-table-column>
      <el-table-column label="创建时间" width="180">
        <template #default="{ row }">
          {{ new Date(row.created_at * 1000).toLocaleString() }}
        </template>
      </el-table-column>
      <el-table-column label="操作">
        <template #default="{ row }">
          <el-button size="small" @click="openResetPwd(row)">重置密码</el-button>
          <el-button size="small"
            :disabled="!row.is_admin && users.filter((u) => u.is_admin).length <= 1"
            @click="toggleAdmin(row)">
            {{ row.is_admin ? '降为普通用户' : '提升为管理员' }}
          </el-button>
          <el-popconfirm
            :title="row.is_admin && users.filter((u) => u.is_admin).length <= 1
              ? '至少保留一个管理员'
              : `确认删除用户「${row.username}」？`"
            @confirm="remove(row)">
            <template #reference>
              <el-button size="small" type="danger"
                :disabled="row.is_admin && users.filter((u) => u.is_admin).length <= 1">
                删除
              </el-button>
            </template>
          </el-popconfirm>
        </template>
      </el-table-column>
    </el-table>

    <!-- 新建用户 -->
    <el-dialog v-model="createVisible" title="新建用户" width="440px" @closed="resetFormState">
      <el-form :model="form" label-width="100px">
        <el-form-item label="用户名">
          <el-input v-model="form.username" placeholder="登录用户名" />
        </el-form-item>
        <el-form-item label="密码">
          <el-input v-model="form.password" type="password" show-password
            placeholder="至少 8 位" />
        </el-form-item>
        <el-form-item label="角色">
          <el-switch v-model="form.is_admin" active-text="管理员" inactive-text="普通用户" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="createVisible = false">取消</el-button>
        <el-button type="primary" :loading="saving" @click="submitCreate">创建</el-button>
      </template>
    </el-dialog>

    <!-- 重置密码 -->
    <el-dialog v-model="resetVisible" title="重置密码" width="440px">
      <el-form :model="resetForm" label-width="100px">
        <el-form-item label="用户">
          <span>{{ resetTarget?.username }}</span>
        </el-form-item>
        <el-form-item label="新密码">
          <el-input v-model="resetForm.password" type="password" show-password
            placeholder="至少 8 位" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="resetVisible = false">取消</el-button>
        <el-button type="primary" :loading="resetting" @click="submitReset">保存</el-button>
      </template>
    </el-dialog>
  </el-card>
</template>

<script setup>
import { onMounted, reactive, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { listUsers, createUser, updateUser, deleteUser } from '../api'

const users = ref([])
const loading = ref(false)
const saving = ref(false)
const resetting = ref(false)

const createVisible = ref(false)
const resetVisible = ref(false)
const resetTarget = ref(null)

const emptyForm = () => ({ username: '', password: '', is_admin: false })
const form = ref(emptyForm())
const resetForm = reactive({ password: '' })

async function load() {
  loading.value = true
  try {
    users.value = await listUsers()
  } finally {
    loading.value = false
  }
}

function openCreate() {
  form.value = emptyForm()
  createVisible.value = true
}

async function submitCreate() {
  const username = form.value.username.trim()
  if (!username) {
    ElMessage.warning('请输入用户名')
    return
  }
  if (form.value.password.length < 8) {
    ElMessage.warning('密码至少 8 位')
    return
  }
  saving.value = true
  try {
    await createUser({
      username,
      password: form.value.password,
      is_admin: form.value.is_admin,
    })
    ElMessage.success('已创建')
    createVisible.value = false
    await load()
  } finally {
    saving.value = false
  }
}

function openResetPwd(row) {
  resetTarget.value = row
  resetForm.password = ''
  resetVisible.value = true
}

async function submitReset() {
  if (resetForm.password.length < 8) {
    ElMessage.warning('密码至少 8 位')
    return
  }
  resetting.value = true
  try {
    await updateUser(resetTarget.value.id, { password: resetForm.password })
    ElMessage.success('密码已重置')
    resetVisible.value = false
  } finally {
    resetting.value = false
  }
}

async function toggleAdmin(row) {
  const next = !row.is_admin
  await updateUser(row.id, { is_admin: next })
  ElMessage.success(next ? '已提升为管理员' : '已降为普通用户')
  await load()
}

async function remove(row) {
  await deleteUser(row.id)
  ElMessage.success('已删除')
  await load()
}

function resetFormState() {
  form.value = emptyForm()
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