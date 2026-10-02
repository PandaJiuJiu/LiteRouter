<template>
  <el-card>
    <div class="toolbar">
      <span>{{ t('settings.description') }}</span>
    </div>

    <!-- 一个设置一行。以后加设置就往这个列表里追加，布局不用再改。 -->
    <div class="row">
      <div class="row-label">{{ t('settings.language.title') }}</div>
      <el-select :model-value="locale" :disabled="languageSaving"
                 class="row-control" @change="onChange">
        <el-option v-for="l in LANGUAGES" :key="l.value" :label="l.label" :value="l.value" />
      </el-select>
    </div>
    <div class="row-desc">{{ t('settings.language.desc') }}</div>

    <!-- 仅管理员可改日志保留窗口。后端一小时一次的 sweep 在每次循环
         重新读 settings，所以这里的改动最多 ~1 小时后生效。 -->
    <div class="row" v-if="isAdmin">
      <div class="row-label">{{ t('settings.logRetention.title') }}</div>
      <el-select
        :model-value="retentionDays"
        :disabled="retentionSaving"
        class="row-control"
        @change="onRetentionChange"
      >
        <el-option
          v-for="d in retentionPresets"
          :key="d"
          :label="t('settings.logRetention.days', { n: d })"
          :value="d"
        />
      </el-select>
    </div>
    <div class="row-desc" v-if="isAdmin">{{ t('settings.logRetention.desc') }}</div>
  </el-card>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { LANGUAGES } from '../i18n'
import { languageSaving, locale, switchLanguage } from '../language'
import { getLogRetention, setLogRetention } from '../api'
import { session } from '../session'

const { t } = useI18n()

const isAdmin = computed(() => session.isAdmin)

// 逻辑在 src/language.js 里 —— 切换语言的地方只有这一个了。
async function onChange(lang) {
  if (lang === locale.value) return
  await switchLanguage(lang)
}

// Log retention: a numeric preset list returned by the backend, plus the
// current value. The presets are what the admin sees in the dropdown; the
// server validates that `days` is in `1..=90` before persisting, so an
// out-of-band edit can't slip a larger window in.
const retentionDays = ref(7)
const retentionPresets = ref([7, 14, 30, 90])
const retentionSaving = ref(false)

async function loadRetention() {
  try {
    const data = await getLogRetention()
    retentionDays.value = data.days
    retentionPresets.value = data.presets
  } catch (_) {
    // The interceptor surfaces the failure (typically 403 for non-admins).
    // Keep the default preset list — an empty dropdown would be worse than
    // the existing one.
  }
}

async function onRetentionChange(days) {
  retentionSaving.value = true
  try {
    retentionDays.value = await setLogRetention(days)
  } finally {
    retentionSaving.value = false
  }
}

onMounted(() => {
  if (isAdmin.value) loadRetention()
})
</script>

<style scoped>
.toolbar {
  margin-bottom: 16px;
}
/* 设置项两栏：左边标题，右边控件。控件定宽，左边缘对齐，
   这样加第二行的时候不会因为标签长短而抖动。 */
.row {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 10px 0;
  border-top: 1px solid #ebeef5;
}
.row-label {
  width: 160px;
  flex: none;
  font-size: 14px;
  color: #303133;
}
.row-control {
  width: 200px;
}
/* 说明挂在控件下面而不是标题里，一行只放一个设置，标题保持干净。 */
.row-desc {
  padding-bottom: 10px;
  margin-left: 176px;
  font-size: 12px;
  color: #909399;
}
</style>