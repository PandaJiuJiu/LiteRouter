<template>
  <div class="login-wrap">
    <el-card class="login-card">
      <h2>Lite One API</h2>
      <el-input v-model="password" type="password" placeholder="管理员密码" show-password
        @keyup.enter="doLogin" />
      <el-button type="primary" style="width: 100%; margin-top: 16px" :loading="loading"
        @click="doLogin">登录</el-button>
    </el-card>
  </div>
</template>

<script setup>
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { login } from '../api'

const password = ref('')
const loading = ref(false)
const router = useRouter()

async function doLogin() {
  if (!password.value) return
  loading.value = true
  try {
    await login(password.value)
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
