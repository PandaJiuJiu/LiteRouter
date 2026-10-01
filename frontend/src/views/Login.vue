<template>
  <div class="login-wrap">
    <el-card class="login-card">
      <img src="/logo-icon.svg" alt="LiteRouter" class="brand" />
      <h2 class="brand-name">LiteRouter</h2>
      <el-input v-model="username" :placeholder="t('login.username')" autocomplete="username"
        style="margin-bottom: 12px" @keyup.enter="doLogin" />
      <el-input v-model="password" type="password" :placeholder="t('login.password')" show-password
        autocomplete="current-password" @keyup.enter="doLogin" />
      <el-button type="primary" style="width: 100%; margin-top: 16px" :loading="loading"
        @click="doLogin">{{ t('login.submit') }}</el-button>
    </el-card>
  </div>
</template>

<script setup>
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { login } from '../api'
import { resetSession } from '../session'

const { t } = useI18n()
const username = ref('')
const password = ref('')
const loading = ref(false)
const router = useRouter()

async function doLogin() {
  if (!username.value || !password.value) return
  loading.value = true
  try {
    await login(username.value.trim(), password.value)
    // The backend hands back a brand-new session id, which may belong to a
    // different user (or a different admin flag) than the one the shared
    // identity cache is holding. Drop it so Layout re-reads /me.
    resetSession()
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
.brand-name {
  margin: 0 0 20px;
  font-size: 22px;
}
</style>