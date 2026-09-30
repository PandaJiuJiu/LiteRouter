<template>
  <el-container style="height: 100vh">
    <el-aside width="200px" style="border-right: 1px solid #e4e7ed">
      <div class="logo"><img src="/logo.svg" alt="LiteRouter" /></div>
      <el-menu :default-active="$route.path" router>
        <el-menu-item v-if="isAdmin" index="/channels">渠道管理</el-menu-item>
        <el-menu-item index="/tokens">令牌管理</el-menu-item>
        <el-menu-item v-if="isAdmin" index="/mappings">模型路由</el-menu-item>
        <el-menu-item index="/usage">用量统计</el-menu-item>
        <el-menu-item index="/logs">调用日志</el-menu-item>
        <el-menu-item v-if="isAdmin" index="/users">用户管理</el-menu-item>
      </el-menu>
      <div class="user-box">
        <div class="user-name" :title="username">
          {{ username }}<span v-if="isAdmin" class="admin-tag">admin</span>
        </div>
        <div class="user-actions">
          <el-button text size="small" @click="openPwd">改密</el-button>
          <el-button text size="small" type="danger" @click="doLogout">登出</el-button>
        </div>
      </div>
    </el-aside>
    <el-main style="background: #f5f7fa">
      <router-view />
    </el-main>

    <!-- 修改自己密码 -->
    <el-dialog v-model="pwdVisible" title="修改密码" width="420px">
      <el-form :model="pwdForm" label-width="100px">
        <el-form-item label="当前密码">
          <el-input v-model="pwdForm.old" type="password" show-password
            autocomplete="current-password" />
        </el-form-item>
        <el-form-item label="新密码">
          <el-input v-model="pwdForm.new" type="password" show-password
            placeholder="至少 8 位" autocomplete="new-password" />
        </el-form-item>
        <el-form-item label="确认新密码">
          <el-input v-model="pwdForm.confirm" type="password" show-password
            autocomplete="new-password" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="pwdVisible = false">取消</el-button>
        <el-button type="primary" :loading="pwdSaving" @click="submitPwd">保存</el-button>
      </template>
    </el-dialog>
  </el-container>
</template>

<script setup>
import { onMounted, reactive, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { changePassword, logout, me } from '../api'

const router = useRouter()
const username = ref('')
const isAdmin = ref(false)

const pwdVisible = ref(false)
const pwdSaving = ref(false)
const pwdForm = reactive({ old: '', new: '', confirm: '' })

async function loadMe() {
  try {
    const u = await me()
    username.value = u.username
    isAdmin.value = !!u.is_admin
  } catch (_) {
    // 401 etc — interceptor handles redirect
  }
}

function openPwd() {
  pwdForm.old = ''
  pwdForm.new = ''
  pwdForm.confirm = ''
  pwdVisible.value = true
}

async function submitPwd() {
  if (!pwdForm.old) {
    ElMessage.warning('请输入当前密码')
    return
  }
  if (pwdForm.new.length < 8) {
    ElMessage.warning('新密码至少 8 位')
    return
  }
  if (pwdForm.new !== pwdForm.confirm) {
    ElMessage.warning('两次新密码不一致')
    return
  }
  pwdSaving.value = true
  try {
    await changePassword(pwdForm.old, pwdForm.new)
    ElMessage.success('密码已更新')
    pwdVisible.value = false
  } finally {
    pwdSaving.value = false
  }
}

async function doLogout() {
  await logout()
  router.push('/login')
}

onMounted(loadMe)
</script>

<style scoped>
.logo {
  padding: 18px 16px;
  border-bottom: 1px solid #f1f5f9;
}
.logo img {
  display: block;
  width: 100%;
  height: auto;
  max-width: 168px;
}
.user-box {
  position: absolute;
  bottom: 16px;
  left: 16px;
  right: 16px;
  border-top: 1px solid #e4e7ed;
  padding-top: 10px;
}
.user-name {
  font-size: 13px;
  color: #303133;
  margin-bottom: 4px;
  display: flex;
  align-items: center;
  gap: 6px;
}
.admin-tag {
  background: #f0f9eb;
  color: #67c23a;
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 8px;
}
.user-actions {
  display: flex;
  gap: 4px;
}
</style>