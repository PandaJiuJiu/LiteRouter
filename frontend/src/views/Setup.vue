<template>
  <div class="setup-wrap">
    <el-card class="setup-card">
      <img src="/logo-icon.svg" alt="LiteRouter" class="brand" />
      <h2 class="brand-name">LiteRouter</h2>
      <p class="hint">{{ t('setup.hint') }}</p>

      <div class="lang-row">
        <span class="lang-label">{{ t('setup.languageLabel') }}</span>
        <!-- 选项文案恒用各自母语，不跟随当前语言 —— 否则选了英文之后
             就没法认出哪个是"中文"了。 -->
        <el-radio-group v-model="form.language" size="small">
          <el-radio-button v-for="l in LANGUAGES" :key="l.value" :value="l.value">
            {{ l.label }}
          </el-radio-button>
        </el-radio-group>
      </div>

      <el-form :model="form" label-position="top">
        <el-form-item :label="t('setup.username')">
          <el-input v-model="form.username" :placeholder="t('setup.usernamePlaceholder')"
            autocomplete="username" @keyup.enter="submit" />
        </el-form-item>
        <el-form-item :label="t('setup.password')">
          <el-input v-model="form.password" type="password" show-password
            :placeholder="t('setup.passwordPlaceholder')" autocomplete="new-password"
            @keyup.enter="submit" />
        </el-form-item>
        <el-form-item :label="t('setup.confirmPassword')">
          <el-input v-model="form.passwordConfirm" type="password" show-password
            :placeholder="t('setup.confirmPasswordPlaceholder')" autocomplete="new-password"
            @keyup.enter="submit" />
        </el-form-item>
        <el-button type="primary" style="width: 100%" :loading="loading"
          @click="submit">{{ t('setup.submit') }}</el-button>
      </el-form>
    </el-card>
  </div>
</template>

<script setup>
import { reactive, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { useI18n } from 'vue-i18n'
import { LANGUAGES, i18n, setLocale } from '../i18n'
import { setup } from '../api'

const { t } = useI18n()
const router = useRouter()
const loading = ref(false)
// 语言先切到用户选的那个，向导本身才能立刻用他们看得懂的文案提示校验错误。
const form = reactive({
  username: '',
  password: '',
  passwordConfirm: '',
  language: i18n.global.locale.value,
})

async function submit() {
  const username = form.username.trim()
  if (!username) {
    ElMessage.warning(t('common.usernameRequired'))
    return
  }
  if (form.password.length < 8) {
    ElMessage.warning(t('common.passwordMinLength'))
    return
  }
  if (form.password !== form.passwordConfirm) {
    ElMessage.warning(t('setup.passwordMismatch'))
    return
  }
  loading.value = true
  try {
    await setup({ username, password: form.password, language: form.language })
    setLocale(form.language)
    ElMessage.success(t('setup.created'))
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
.brand-name {
  text-align: center;
  margin: 0 0 4px;
  font-size: 22px;
}
.hint {
  font-size: 13px;
  text-align: center;
  margin: 0 0 18px;
}
.lang-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 12px;
  margin-bottom: 18px;
  background: #fafafa;
  border: 1px solid #ebeef5;
  border-radius: 6px;
}
.lang-label {
  font-size: 13px;
  color: #606266;
}
</style>