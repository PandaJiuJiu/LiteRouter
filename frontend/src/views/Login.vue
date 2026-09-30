<template>
  <div class="login-wrap">
    <el-card class="login-card">
      <h2>LiteRouter</h2>
      <el-input v-model="username" placeholder="用户名" autocomplete="username"
        style="margin-bottom: 12px" @keyup.enter="doLogin" />
      <el-input v-model="password" type="password" placeholder="密码" show-password
        autocomplete="current-password" @keyup.enter="doLogin" />
      <el-button type="primary" style="width: 100%; margin-top: 16px" :loading="loading"
        @click="doLogin">登录</el-button>
    </el-card>
  </div>
</template>

<script setup>
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { login } from '../api'

const username = ref('')
const password = ref('')
const loading = ref(false)
const router = useRouter()

async function doLogin() {
  if (!username.value || !password.value) return
  loading.value = true
  try {
    await login(username.value.trim(), password.value)
    router.push('/')
  } finally {
    loading.value = false
  }
}
</script>

<style scoped>
.login-wrap {
  height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #f5f7fa;
}
.login-card {
  width: 360px;
  text-align: center;
}
</style>