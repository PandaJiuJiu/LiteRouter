<template>
  <el-card>
    <div class="toolbar">
      <span>{{ t('settings.description') }}</span>
    </div>

    <section class="section">
      <div class="section-title">{{ t('settings.language.title') }}</div>
      <div class="section-desc">{{ t('settings.language.desc') }}</div>
      <el-radio-group :model-value="locale" @change="onChange" :disabled="languageSaving">
        <el-radio v-for="l in LANGUAGES" :key="l.value" :value="l.value" class="lang-option">
          {{ l.label }}
        </el-radio>
      </el-radio-group>
    </section>
  </el-card>
</template>

<script setup>
import { useI18n } from 'vue-i18n'
import { LANGUAGES } from '../i18n'
import { languageSaving, locale, switchLanguage } from '../language'

const { t } = useI18n()

// 逻辑在 src/language.js 里，侧边栏下拉走的是同一个函数 —— 两处必须一致。
async function onChange(lang) {
  if (lang === locale.value) return
  await switchLanguage(lang)
}
</script>

<style scoped>
.toolbar {
  margin-bottom: 16px;
}
.section {
  border-top: 1px solid #ebeef5;
  padding-top: 16px;
}
.section-title {
  font-size: 14px;
  font-weight: 600;
  color: #303133;
  margin-bottom: 4px;
}
.section-desc {
  font-size: 12px;
  color: #909399;
  margin-bottom: 12px;
}
.lang-option {
  margin-bottom: 8px;
}
</style>