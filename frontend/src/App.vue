<template>
  <el-config-provider :locale="elLocale">
    <router-view />
  </el-config-provider>
</template>

<script setup>
import { computed } from 'vue'
import zhCn from 'element-plus/es/locale/lang/zh-cn'
import en from 'element-plus/es/locale/lang/en'
import { i18n } from './i18n'

// Element Plus 自带文案（分页的"上一页"、popconfirm 的默认按钮、日期选择器…）。
// 不通过 config-provider 传 locale 的话，这些会永远是中文，跟自绘的对不上。
const elLocale = computed(() => (i18n.global.locale.value === 'en-US' ? en : zhCn))
</script>