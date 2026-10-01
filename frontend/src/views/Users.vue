<template>
  <el-card>
    <div class="toolbar">
      <span>{{ t('users.description') }}</span>
      <el-button type="primary" @click="openCreate">{{ t('users.create') }}</el-button>
    </div>

    <el-table :data="users" v-loading="loading">
      <el-table-column prop="username" :label="t('users.col.username')" width="200" />
      <el-table-column :label="t('users.col.role')" width="140">
        <template #default="{ row }">
          <el-tag v-if="row.is_admin" type="success" size="small">{{ t('users.roleAdmin') }}</el-tag>
          <el-tag v-else size="small">{{ t('users.roleUser') }}</el-tag>
        </template>
      </el-table-column>
      <el-table-column :label="t('users.col.created')" width="180">
        <template #default="{ row }">
          {{ new Date(row.created_at * 1000).toLocaleString() }}
        </template>
      </el-table-column>
      <el-table-column :label="t('users.col.actions')">
        <template #default="{ row }">
          <el-button size="small" @click="openResetPwd(row)">{{ t('users.resetPassword') }}</el-button>
          <el-popconfirm
            :title="row.is_admin && users.filter((u) => u.is_admin).length <= 1
              ? t('users.keepOneAdmin')
              : t('users.deleteConfirm', { username: row.username })"
            @confirm="remove(row)">
            <template #reference>
              <el-button size="small" type="danger"
                :disabled="row.is_admin && users.filter((u) => u.is_admin).length <= 1">
                {{ t('common.delete') }}
              </el-button>
            </template>
          </el-popconfirm>
        </template>
      </el-table-column>
    </el-table>

    <!-- 新建用户 -->
    <el-dialog v-model="createVisible" :title="t('users.createTitle')" width="440px" @closed="resetFormState">
      <el-form :model="form" label-width="100px">
        <el-form-item :label="t('users.col.username')">
          <el-input v-model="form.username" :placeholder="t('users.usernamePlaceholder')" />
        </el-form-item>
        <el-form-item :label="t('login.password')">
          <el-input v-model="form.password" type="password" show-password
            :placeholder="t('common.min8Chars')" />
        </el-form-item>
        <el-form-item :label="t('users.roleLabel')">
          <el-switch v-model="form.is_admin" :active-text="t('users.roleAdmin')"
            :inactive-text="t('users.roleUser')" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="createVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" :loading="saving" @click="submitCreate">{{ t('common.create') }}</el-button>
      </template>
    </el-dialog>

    <!-- 重置密码 -->
    <el-dialog v-model="resetVisible" :title="t('users.resetTitle')" width="440px">
      <el-form :model="resetForm" label-width="100px">
        <el-form-item :label="t('users.userLabel')">
          <span>{{ resetTarget?.username }}</span>
        </el-form-item>
        <el-form-item :label="t('users.newPasswordLabel')">
          <el-input v-model="resetForm.password" type="password" show-password
            :placeholder="t('common.min8Chars')" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="resetVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" :loading="resetting" @click="submitReset">{{ t('common.save') }}</el-button>
      </template>
    </el-dialog>
  </el-card>
</template>

<script setup>
import { onMounted, reactive, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { useI18n } from 'vue-i18n'
import { listUsers, createUser, updateUser, deleteUser } from '../api'

const { t } = useI18n()

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
    ElMessage.warning(t('common.usernameRequired'))
    return
  }
  if (form.value.password.length < 8) {
    ElMessage.warning(t('common.passwordMinLength'))
    return
  }
  saving.value = true
  try {
    await createUser({
      username,
      password: form.value.password,
      is_admin: form.value.is_admin,
    })
    ElMessage.success(t('users.created'))
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
    ElMessage.warning(t('common.passwordMinLength'))
    return
  }
  resetting.value = true
  try {
    await updateUser(resetTarget.value.id, { password: resetForm.password })
    ElMessage.success(t('users.passwordReset'))
    resetVisible.value = false
  } finally {
    resetting.value = false
  }
}

async function remove(row) {
  await deleteUser(row.id)
  ElMessage.success(t('users.deleted'))
  await load()
}

function resetFormState() {
  form.value = emptyForm()
}

onMounted(load)
</script>

<style scoped>
</style>