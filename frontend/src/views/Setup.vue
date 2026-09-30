<template>
  <div class="setup-wrap">
    <el-card class="setup-card">
      <h2>LiteRouter</h2>
      <p class="hint">首次使用，请创建管理员账号</p>
      <el-form :model="form" label-position="top">
        <el-form-item label="用户名">
          <el-input v-model="form.username" placeholder="管理员用户名" autocomplete="username"
            @keyup.enter="submit" />
        </el-form-item>
        <el-form-item label="密码（至少 8 位）">
          <el-input v-model="form.password" type="password" show-password
            placeholder="密码" autocomplete="new-password" @keyup.enter="submit" />
        </el-form-item>
        <el-form-item label="确认密码">
          <el-input v-model="form.passwordConfirm" type="password" show-password
            placeholder="再输入一次" autocomplete="new-password" @keyup.enter="submit" />
        </el-form-item>
        <el-button type="primary" style="width: 100%" :loading="loading"
          @click="submit">创建管理员并进入</el-button>
      </el-form>
    </el-card>
  </div>
</template>

<script setup>
import { reactive, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { setup } from '../api'

const router = useRouter()
const loading = ref(false)
const form = reactive({ username: '', password: '', passwordConfirm: '' })

async function submit() {
  const username = form.username.trim()
  if (!username) {
    ElMessage.warning('请输入用户名')
    return
  }
  if (form.password.length < 8) {
    ElMessage.warning('密码至少 8 位')
    return
  }
  if (form.password !== form.passwordConfirm) {
    ElMessage.warning('两次密码不一致')
    return
  }
  loading.value = true
  try {
    await setup({ username, password: form.password })
    ElMessage.success('已创建，正在进入控制台')
    router.push('/channels')
  } finally {
    loading.value = false
  }
}
</script>

<style scoped>
.setup-wrap {
  height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #f5f7fa;
}
.setup-card {
  width: 380px;
}
.setup-card h2 {
  margin: 0 0 4px;
  text-align: center;
}
.hint {
  color: #909399;
  font-size: 13px;
  text-align: center;
  margin: 0 0 18px;
}
</style>