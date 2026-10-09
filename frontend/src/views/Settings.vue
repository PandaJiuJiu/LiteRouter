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

    <!-- 网络代理：分两行——
         第一行：代理服务器地址（host/port）
         第二行：全局代理开关。开启后所有渠道都走代理；关闭时只有渠道页勾选
         「使用代理」的渠道走代理。两者是 OR 关系，互不依赖。 -->
    <div class="row" v-if="isAdmin">
      <div class="row-label">{{ t('settings.proxy.title') }}</div>
      <div class="row-control row-actions" style="gap: 12px; width: auto;">
        <el-input
          v-model="proxyHost"
          :disabled="proxySaving"
          :placeholder="t('settings.proxy.hostPlaceholder')"
          style="width: 200px;"
          @change="onProxyChange"
        />
        <el-input-number
          v-model="proxyPort"
          :disabled="proxySaving"
          :min="1"
          :max="65535"
          :placeholder="t('settings.proxy.portPlaceholder')"
          style="width: 120px;"
          @change="onProxyChange"
        />
      </div>
    </div>
    <div class="row-desc" v-if="isAdmin">{{ t('settings.proxy.hostDesc') }}</div>

    <div class="row" v-if="isAdmin">
      <div class="row-label">{{ t('settings.proxy.enableTitle') }}</div>
      <div class="row-control row-actions" style="gap: 12px; width: auto;">
        <el-switch
          v-model="proxyEnabled"
          :disabled="proxySaving"
          :loading="proxySaving"
          @change="onProxyEnabledChange"
        />
        <span class="proxy-enabled-label">{{
          proxyEnabled ? t('settings.proxy.enabled') : t('settings.proxy.disabled')
        }}</span>
      </div>
    </div>
    <div class="row-desc" v-if="isAdmin">{{ t('settings.proxy.enableDesc') }}</div>

    <!-- 备份 / 还原只是一个入口行——所有交互（段位、密码、文件、预览、
         冲突解决、提交统计）都在 ConfigBackup 的弹窗里。 -->
    <div class="row" v-if="isAdmin">
      <div class="row-label">{{ t('settings.backup.title') }}</div>
      <div class="row-control row-actions">
        <el-button @click="importDialogVisible = true">
          {{ t('settings.backup.buttons.import') }}
        </el-button>
        <el-button type="primary" @click="exportDialogVisible = true">
          {{ t('settings.backup.buttons.export') }}
        </el-button>
      </div>
    </div>
    <div class="row-desc" v-if="isAdmin">{{ t('settings.backup.desc') }}</div>

    <ConfigBackup
      v-if="isAdmin"
      v-model:exportVisible="exportDialogVisible"
      v-model:importVisible="importDialogVisible"
    />

    <!-- 当前系统版本：后端 Cargo.toml 的版本号，由 /api/setup-status 透出。
         所有用户可见，纯展示，没有交互。 -->
    <div class="row">
      <div class="row-label">{{ t('settings.version.title') }}</div>
      <div class="row-control row-version">{{ version ? 'v' + version : '—' }}</div>
    </div>
  </el-card>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { LANGUAGES } from '../i18n'
import { languageSaving, locale, switchLanguage } from '../language'
import { getLogRetention, setLogRetention, getProxySettings, setProxySettings, setupStatus } from '../api'
import { session } from '../session'
import ConfigBackup from './ConfigBackup.vue'

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

// Proxy settings: host + port describe the proxy server; `enabled` is the
// independent global on/off switch. All three are partial-updatable: host
// or port change fires when the input commits; the switch fires on toggle.
const proxyHost = ref('')
const proxyPort = ref(0)
const proxyEnabled = ref(false)
const proxySaving = ref(false)

async function loadProxy() {
  try {
    const data = await getProxySettings()
    proxyHost.value = data.host || ''
    proxyPort.value = data.port || 0
    proxyEnabled.value = !!data.enabled
  } catch (_) {
    // ignore
  }
}

async function onProxyChange() {
  proxySaving.value = true
  try {
    const data = await setProxySettings({
      host: proxyHost.value,
      port: proxyPort.value,
    })
    proxyHost.value = data.host || ''
    proxyPort.value = data.port || 0
  } finally {
    proxySaving.value = false
  }
}

async function onProxyEnabledChange(value) {
  proxySaving.value = true
  try {
    const data = await setProxySettings({ enabled: value })
    proxyEnabled.value = !!data.enabled
  } finally {
    proxySaving.value = false
  }
}

// Both dialogs are controlled here so closing them from inside the child
// (`invalidates clear here`) and reopening from these buttons stays in
// sync via v-model.
const exportDialogVisible = ref(false)
const importDialogVisible = ref(false)

// 系统版本号，来自 /api/setup-status。取不到（后端还没起来）就显示占位横线。
const version = ref('')

onMounted(() => {
  if (isAdmin.value) {
    loadRetention()
    loadProxy()
  }
  setupStatus()
    .then((s) => {
      version.value = s?.version ?? ''
    })
    .catch(() => {
      version.value = ''
    })
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
.row-actions {
  display: flex;
  gap: 8px;
  width: auto;
}
/* 说明挂在控件下面而不是标题里，一行只放一个设置，标题保持干净。 */
.row-desc {
  padding-bottom: 10px;
  margin-left: 176px;
  font-size: 12px;
  color: #909399;
}
/* 版本号是只读文本，不是控件 —— 用弱化色和等宽字体把它跟可改的设置区分开。 */
.row-version {
  font-family: monospace;
  font-size: 13px;
  color: #909399;
}
</style>