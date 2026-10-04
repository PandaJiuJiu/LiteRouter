<template>
  <el-card v-loading="loading">
    <div class="toolbar">
      <div class="toolbar-title">
        <span v-if="channelId">{{ t('models.headerForChannel', { name: channelName }) }}</span>
        <template v-else>
          <span class="title">{{ t('models.pageTitle') }}</span>
          <span class="subtitle">{{ t('models.subtitle') }}</span>
        </template>
      </div>
      <div class="toolbar-actions">
        <el-button v-if="channelId" @click="back">{{ t('models.backToChannels') }}</el-button>
        <el-button v-else @click="load">{{ t('common.refresh') }}</el-button>
      </div>
    </div>

    <div v-if="!channelId" class="filters">
      <el-input
        v-model="search"
        :placeholder="t('models.searchPlaceholder')"
        clearable
        size="small"
        class="filter-search"
      />
      <el-select
        v-model="filterChannelId"
        :placeholder="t('models.filterByChannel')"
        clearable
        size="small"
        class="filter-channel"
      >
        <el-option v-for="ch in channels" :key="ch.id" :label="ch.name" :value="ch.id" />
      </el-select>
      <span class="filter-toggle">
        <span class="filter-toggle-label">{{ t('models.showEnabledOnly') }}</span>
        <el-switch v-model="enabledOnly" />
      </span>
      <div class="summary">
        <el-tag size="small" effect="plain">{{ t('models.summaryModels', { count: totalModels }) }}</el-tag>
        <el-tag size="small" effect="plain" type="info">{{ t('models.summaryChannels', { count: channels.length }) }}</el-tag>
        <el-tag v-if="totalDisabled" size="small" effect="plain" type="warning">
          {{ t('models.summaryDisabled', { count: totalDisabled }) }}
        </el-tag>
      </div>
    </div>

    <el-empty v-if="!channels.length && !loading" :description="t('models.emptyNoChannels')" />
    <el-empty
      v-else-if="!channelId && !matchedChannels.length"
      :description="t('models.noMatchAll')"
    />

    <el-card v-for="ch in matchedChannels" :key="ch.id" class="channel-card" shadow="never">
      <div class="channel-head">
        <div>
          <span class="channel-name">{{ ch.name }}</span>
          <el-tag v-if="ch.enabled" type="success" size="small">{{ t('common.enabled') }}</el-tag>
          <el-tag v-else type="danger" size="small">{{ t('common.disabled') }}</el-tag>
          <el-tag v-if="ch.base_url" size="small">OpenAI</el-tag>
          <el-tag v-if="ch.base_url_anthropic" size="small">Anthropic</el-tag>
        </div>
        <div>
          <el-button size="small" :loading="ch._testingAll"
            :disabled="!enabledModelList(ch).length" @click="testAll(ch)">
            {{ t('models.testAll') }}
          </el-button>
          <el-button size="small" :loading="ch._fetching"
            :disabled="!ch.base_url && !ch.base_url_anthropic" @click="fetchList(ch)">
            {{ t('models.fetchList') }}
          </el-button>
          <el-button size="small" @click="openManual(ch)">{{ t('models.manualAdd') }}</el-button>
          <el-button
            size="small"
            v-if="!channelId"
            @click="router.push('/models?channel=' + ch.id)"
          >{{ t('models.focusChannel') }}</el-button>
        </div>
      </div>

      <div v-if="!ch.base_url" class="hint">
        {{ t('models.anthropicOnlyHint') }}
      </div>

      <!-- 模型卡片网格 -->
      <div v-if="modelList(ch).length" class="model-grid">
        <div v-for="m in modelList(ch)" :key="m" class="model-card"
             :class="{ 'is-enabled': isSelected(ch, m), 'is-disabled': !isSelected(ch, m), 'is-testing': ch._testingModel === m }">
          <div class="model-card-top">
            <div class="model-name" :title="m">{{ m }}</div>
            <el-switch :model-value="isSelected(ch, m)"
                       :loading="ch._togglingModel === m"
                       @change="(v) => toggleModel(ch, m, v)" />
          </div>
          <div class="model-card-status">
            <template v-for="proto in PROTOCOLS" :key="proto">
              <template v-if="ch._testResult?.[m]?.protocols?.[proto]">
                <el-tooltip v-if="ch._testResult[m].protocols[proto].ok === false"
                            :content="ch._testResult[m].protocols[proto].error || t('models.unavailable')"
                            placement="top">
                  <div class="status-line fail">
                    <el-icon class="status-icon"><CircleCloseFilled /></el-icon>
                    <span class="status-proto">{{ proto }}</span>
                    <span class="status-text">{{ statusText(ch, m, proto) }}</span>
                  </div>
                </el-tooltip>
                <div v-else class="status-line ok">
                  <el-icon class="status-icon"><CircleCheckFilled /></el-icon>
                  <span class="status-proto">{{ proto }}</span>
                  <span class="status-text">{{ statusText(ch, m, proto) }}</span>
                </div>
              </template>
            </template>
            <div v-if="!hasTestResult(ch, m)" class="status-untested">{{ t('models.untested') }}</div>
          </div>
          <div class="model-card-actions">
            <el-button class="action-btn" :loading="ch._testingModel === m"
                       @click="pingModel(ch, m)">
              <el-icon><Refresh /></el-icon>
              <span>{{ t('models.test') }}</span>
            </el-button>
            <el-popconfirm class="action-pop"
                           :title="t('models.removeConfirm', { channel: ch.name, model: m })"
                           :confirm-button-text="t('common.remove')"
                           :cancel-button-text="t('common.cancel')"
                           @confirm="removeModel(ch, m)">
              <template #reference>
                <el-button class="action-btn" type="danger" plain>
                  <el-icon><Delete /></el-icon>
                  <span>{{ t('common.delete') }}</span>
                </el-button>
              </template>
            </el-popconfirm>
          </div>
        </div>
      </div>
      <span v-else class="hint">{{ t('models.noModels') }}</span>
    </el-card>

    <!-- 手动添加模型 -->
    <el-dialog v-model="manualVisible" :title="t('models.manualTitle')" width="480px">
      <p class="hint" style="margin-top: 0">{{ t('models.manualHint') }}</p>
      <el-input v-model="manualInput" type="textarea" :rows="4" :placeholder="t('models.manualPlaceholder')" />
      <template #footer>
        <el-button @click="manualVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" @click="saveManual">{{ t('common.save') }}</el-button>
      </template>
    </el-dialog>

    <!-- 选择要添加的模型 -->
    <el-dialog v-model="selectVisible"
      :title="t('models.selectTitle', { channel: selectTarget?.name || '' })"
      width="560px" top="6vh" @closed="onSelectClosed">
      <div class="select-toolbar">
        <span class="hint">{{ t('models.selectedCount', { selected: selectedArr.length, total: fetchedList.length }) }}</span>
        <div>
          <el-button size="small" @click="selectAll">{{ t('models.selectAll') }}</el-button>
          <el-button size="small" @click="selectNone">{{ t('models.selectNone') }}</el-button>
          <el-button size="small" @click="selectInverse">{{ t('models.selectInverse') }}</el-button>
        </div>
      </div>
      <el-input v-model="selectFilter" :placeholder="t('models.filterPlaceholder')"
        clearable class="select-filter" />
      <div class="select-list">
        <el-checkbox-group v-model="selectedArr">
          <el-checkbox v-for="m in filteredList" :key="m" :value="m" class="select-row">
            <span class="select-name">{{ m }}</span>
          </el-checkbox>
        </el-checkbox-group>
        <el-empty v-if="!filteredList.length" :description="t('models.noMatch')" :image-size="60" />
      </div>
      <template #footer>
        <el-button @click="selectVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" :loading="selectSaving" @click="confirmSelect">
          {{ t('models.addToChannel') }}
        </el-button>
      </template>
    </el-dialog>
  </el-card>
</template>

<script setup>
import { computed, onMounted, reactive, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { useI18n } from 'vue-i18n'
import { CircleCheckFilled, CircleCloseFilled, Delete, Refresh } from '@element-plus/icons-vue'
import { listChannels, updateChannelModels, fetchModels, testModel } from '../api'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
// 从渠道列表点"模型管理"进入时只看该渠道
const channelId = computed(() => (route.query.channel ? Number(route.query.channel) : null))
const channelName = computed(
  () => channels.value.find((c) => c.id === channelId.value)?.name || '',
)

function back() {
  router.push('/channels')
}

const channels = ref([])
const loading = ref(false)
// known[id] = 该渠道出现过的所有模型名（启用 ∪ 停用 ∪ 上游拉取 ∪ 手动添加 的并集）
// 切换启用/停用把模型在 ch.models 和 ch.disabled_models 之间挪动，不动 known —
// 刷新页面后，停用的卡片还在，可以再开回来。删除按钮才会从 known 里抹掉。
const known = reactive({})

// Toolbar filters — only meaningful on the un-focused view. `search` matches
// against either enabled or disabled models; `enabledOnly` narrows to
// channels that actually have an enabled entry; `filterChannelId` pins the
// list to one channel without leaving the page. All three are local, no URL
// state: a refresh resets them, which is the right call for a debugging page.
const search = ref('')
const filterChannelId = ref(null)
const enabledOnly = ref(false)

function channelModelStrings(ch) {
  // Models that this channel *could* show — enabled + disabled, normalized
  // (trim, dedup, drop empty / '*').
  const all = [...splitModels(ch.models), ...splitModels(ch.disabled_models)]
  return [...new Set(all.map((m) => m.trim()).filter((m) => m && m !== '*'))]
}

function channelHasEnabled(ch) {
  return splitModels(ch.models).length > 0
}

const matchedChannels = computed(() => {
  // Pinned by the query string → skip the filter, render just that one.
  if (channelId.value) {
    return channels.value.filter((c) => c.id === channelId.value)
  }
  const q = search.value.trim().toLowerCase()
  return channels.value.filter((ch) => {
    if (filterChannelId.value && ch.id !== filterChannelId.value) return false
    if (enabledOnly.value && !channelHasEnabled(ch)) return false
    if (q) {
      const haystack = channelModelStrings(ch).join('\n').toLowerCase()
      if (!haystack.includes(q)) return false
    }
    return true
  })
})

// Toolbar chips — counts taken across every channel, not the filtered subset, so
// the admin sees the whole picture at a glance and the chips don't flicker
// when they type in the search box.
const totalModels = computed(() =>
  channels.value.reduce((acc, ch) => acc + channelModelStrings(ch).length, 0),
)
const totalDisabled = computed(() =>
  channels.value.reduce((acc, ch) => acc + splitModels(ch.disabled_models).length, 0),
)

const manualVisible = ref(false)
const manualInput = ref('')
const manualTarget = ref(null)

// "选择要添加的模型" 对话框状态
const selectVisible = ref(false)
const selectTarget = ref(null)
const fetchedList = ref([])
const selectedArr = ref([])
const selectFilter = ref('')
const selectSaving = ref(false)

const filteredList = computed(() => {
  const f = selectFilter.value.trim().toLowerCase()
  if (!f) return fetchedList.value
  return fetchedList.value.filter((m) => m.toLowerCase().includes(f))
})

function ensureKnown(chId) {
  if (!known[chId]) known[chId] = []
  return known[chId]
}

function rememberModels(chId, models) {
  const set = ensureKnown(chId)
  const before = set.length
  for (const m of models) if (!set.includes(m)) set.push(m)
  return set.length - before
}

function modelList(ch) {
  return known[ch.id] || []
}

// 只包含启用中的模型 —— 「全部测试」和按钮可用状态都基于它。
function enabledModelList(ch) {
  return modelList(ch).filter((m) => isSelected(ch, m))
}

function splitModels(s) {
  return (s || '').split(',').map((x) => x.trim()).filter((x) => x && x !== '*')
}

const PROTOCOLS = ['openai', 'anthropic']

function hasTestResult(ch, m) {
  const r = ch._testResult?.[m]
  return r && r.protocols && Object.keys(r.protocols).length > 0
}

// 「可用 / 不可用」这个词在颜色和图标上已经说过了；真正新增的信息是耗时，
// 所以这里只报耗时，慢的那次一眼能看出来。
function statusText(ch, m, proto) {
  const ms = ch._testResult?.[m]?.protocols?.[proto]?.ms
  if (typeof ms !== 'number') return '—'
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(2)} s`
}

function isSelected(ch, m) {
  return splitModels(ch.models).includes(m)
}

async function load() {
  loading.value = true
  try {
    const all = await listChannels()
    channels.value = channelId.value ? all.filter((c) => c.id === channelId.value) : all
    // seed known list from enabled ∪ disabled so disabled cards survive refresh
    for (const ch of channels.value) {
      rememberModels(ch.id, [
        ...splitModels(ch.models),
        ...splitModels(ch.disabled_models),
      ])
    }
  } finally {
    loading.value = false
  }
}

async function fetchList(ch) {
  ch._fetching = true
  try {
    const { models } = await fetchModels({
      base_url: ch.base_url,
      base_url_anthropic: ch.base_url_anthropic,
      api_key: ch.api_key,
      use_proxy: ch.use_proxy,
    })
    if (!models.length) {
      ElMessage.warning(t('models.emptyUpstream'))
      return
    }
    // 去重 + 排序，让列表更易扫
    const unique = [...new Set(models)].sort()
    // 默认勾选 = 当前渠道已配置的 ∩ 这次拉到的
    const existing = new Set(splitModels(ch.models))
    selectTarget.value = ch
    fetchedList.value = unique
    selectedArr.value = unique.filter((m) => existing.has(m))
    selectFilter.value = ''
    selectVisible.value = true
  } finally {
    ch._fetching = false
  }
}

function selectAll() {
  // 在当前过滤范围内追加，避免覆盖已勾选但被过滤掉的项
  const set = new Set(selectedArr.value)
  for (const m of filteredList.value) set.add(m)
  selectedArr.value = [...set]
}

function selectNone() {
  // 仅清掉当前过滤范围内已勾选的，保留被过滤掉的项
  const filtered = new Set(filteredList.value)
  selectedArr.value = selectedArr.value.filter((m) => !filtered.has(m))
}

function selectInverse() {
  const filtered = new Set(filteredList.value)
  const current = new Set(selectedArr.value)
  const next = new Set()
  // 未在过滤范围内的保持原样
  for (const m of current) if (!filtered.has(m)) next.add(m)
  // 过滤范围内反选
  for (const m of filtered) if (!current.has(m)) next.add(m)
  selectedArr.value = [...next]
}

function onSelectClosed() {
  // 关闭后清掉临时状态，避免下次打开残留
  fetchedList.value = []
  selectedArr.value = []
  selectFilter.value = ''
  selectTarget.value = null
}

async function confirmSelect() {
  const ch = selectTarget.value
  if (!ch) {
    selectVisible.value = false
    return
  }
  // 拍下旧值快照，后面要算"新增多少"
  const oldModels = splitModels(ch.models)
  const oldDisabled = splitModels(ch.disabled_models)
  const merged = [...new Set([...oldModels, ...selectedArr.value])]
  // 这次被勾选上的、本来在 disabled 里的模型 → 移到 enabled,
  // 从 disabled 集合里抹掉。两个集合保持互斥。
  const newDisabled = oldDisabled.filter((m) => !merged.includes(m))
  const newModels = merged.join(',')
  const newDisabledStr = newDisabled.join(',')
  // 没变化就不写库，但弹个提示让用户知道发生了什么
  if (newModels === ch.models && newDisabledStr === ch.disabled_models) {
    selectVisible.value = false
    ElMessage.info(t('models.noChanges'))
    return
  }
  selectSaving.value = true
  try {
    await updateChannelModels(ch.id, newModels, newDisabledStr)
    ch.models = newModels
    ch.disabled_models = newDisabledStr
    // 只把"真正要在这个渠道里"的模型加入 known——上游拉到的全部模型只
    // 是候选，未勾选的就不该出现卡片，否则刷新后也没了。
    // rememberModels 自身去重，selectedArr 里已存在的旧模型是 no-op。
    rememberModels(ch.id, selectedArr.value)
    const oldSet = new Set(oldModels)
    const added = selectedArr.value.filter((m) => !oldSet.has(m)).length
    ElMessage.success(
      added > 0
        ? t('models.addedModels', { count: added, channel: ch.name })
        : t('models.modelsUpdated', { channel: ch.name }),
    )
    selectVisible.value = false
  } catch (e) {
    const detail = e?.response?.data?.message || e.message || t('common.unknownError')
    ElMessage.error(t('models.saveFailed', { detail }))
  } finally {
    selectSaving.value = false
  }
}

async function toggleModel(ch, m, enabled) {
  const enabledSet = new Set(splitModels(ch.models))
  const disabledSet = new Set(splitModels(ch.disabled_models))
  if (enabled) {
    enabledSet.add(m)
    disabledSet.delete(m)
  } else {
    enabledSet.delete(m)
    disabledSet.add(m)
  }
  const newModels = [...enabledSet].join(',')
  const newDisabled = [...disabledSet].join(',')
  if (newModels === ch.models && newDisabled === ch.disabled_models) return
  ch._togglingModel = m
  try {
    await updateChannelModels(ch.id, newModels, newDisabled)
    ch.models = newModels
    ch.disabled_models = newDisabled
  } catch (e) {
    ElMessage.error(t('models.updateModelFailed', { model: m }))
  } finally {
    ch._togglingModel = null
  }
}

// 完全删除：从 ch.models 和 ch.disabled_models 都移除并持久化，
// 从 known 移除（卡片消失），测试结果也清掉。要恢复只能重新获取或手动添加。
async function removeModel(ch, m) {
  // 1. 持久化（如果该模型在任一集合中）
  const cur = splitModels(ch.models)
  const curDisabled = splitModels(ch.disabled_models)
  const inSaved = cur.includes(m) || curDisabled.includes(m)
  if (inSaved) {
    const newModels = cur.filter((x) => x !== m).join(',')
    const newDisabled = curDisabled.filter((x) => x !== m).join(',')
    try {
      await updateChannelModels(ch.id, newModels, newDisabled)
      ch.models = newModels
      ch.disabled_models = newDisabled
    } catch (e) {
      const detail = e?.response?.data?.message || e.message || t('common.unknownError')
      ElMessage.error(t('models.removeFailed', { model: m, detail }))
      return  // 写库失败就不动 known，避免前端状态和数据库脱钩
    }
  }
  // 2. 从 known 移除，让卡片消失
  const list = known[ch.id]
  if (list) {
    const idx = list.indexOf(m)
    if (idx >= 0) list.splice(idx, 1)
  }
  // 3. 清掉残留的测试结果，避免下次同名的卡片（罕见）读到旧状态
  if (ch._testResult) delete ch._testResult[m]
  ElMessage.success(t('models.removed', { channel: ch.name, model: m }))
}

async function pingModel(ch, m) {
  ch._testingModel = m
  try {
    const r = await testModel({
      base_url: ch.base_url,
      base_url_anthropic: ch.base_url_anthropic,
      api_key: ch.api_key,
      model: m,
      use_proxy: ch.use_proxy,
    })
    if (!ch._testResult) ch._testResult = {}
    ch._testResult[m] = r
    // toast summary
    const ok = PROTOCOLS.filter((p) => r.protocols?.[p]?.ok)
    if (r.ok) ElMessage.success(t('models.testOk', { model: m, protocols: ok.join(' + ') || t('models.available') }))
    else ElMessage.error(t('models.testUnavailable', { model: m }))
  } catch (e) {
    if (!ch._testResult) ch._testResult = {}
    ch._testResult[m] = { ok: false, protocols: {} }
    ElMessage.error(t('models.testFailed', { model: m }))
  } finally {
    ch._testingModel = null
  }
}

async function testAll(ch) {
  // 只测启用的模型：停用的模型本来就不参与路由，测它既没有参考价值，
  // 又会把「全部测试」的等待时间按停用模型的数量线性拉长。
  const list = enabledModelList(ch)
  if (!list.length) return
  ch._testingAll = true
  let ok = 0
  let fail = 0
  try {
    for (const m of list) {
      // piggy-back on pingModel so the per-card spinner and `_testResult`
      // stay consistent with single-model tests. Sequential is intentional:
      // upstream rate-limits often bite on bursts, and one model timing out
      // doesn't block the rest from being judged.
      ch._testingModel = m
      try {
        const r = await testModel({
          base_url: ch.base_url,
          base_url_anthropic: ch.base_url_anthropic,
          api_key: ch.api_key,
          model: m,
          use_proxy: ch.use_proxy,
        })
        if (!ch._testResult) ch._testResult = {}
        ch._testResult[m] = r
        if (r.ok) ok++
        else fail++
      } catch (_) {
        if (!ch._testResult) ch._testResult = {}
        ch._testResult[m] = { ok: false, protocols: {} }
        fail++
      }
    }
    if (fail === 0) ElMessage.success(t('models.allOk', { channel: ch.name, count: ok }))
    else ElMessage.warning(t('models.summary', { channel: ch.name, ok, fail }))
  } finally {
    ch._testingModel = null
    ch._testingAll = false
  }
}

function openManual(ch) {
  manualTarget.value = ch
  manualInput.value = ''
  manualVisible.value = true
}

async function saveManual() {
  const ch = manualTarget.value
  const adds = splitModels(manualInput.value)
  if (!adds.length) {
    ElMessage.warning(t('models.needOneModel'))
    return
  }
  const enabled = [...new Set([...splitModels(ch.models), ...adds])]
  // 手动添加等于"启用"，所以原来在 disabled 里的同名模型也要挪出来
  const disabled = splitModels(ch.disabled_models).filter((m) => !enabled.includes(m))
  const models = enabled.join(',')
  const disabledStr = disabled.join(',')
  await updateChannelModels(ch.id, models, disabledStr)
  ch.models = models
  ch.disabled_models = disabledStr
  rememberModels(ch.id, adds)
  manualVisible.value = false
  ElMessage.success(t('models.manualAdded', { count: adds.length }))
}

onMounted(load)
</script>

<style scoped>
.toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 12px;
  margin-bottom: 12px;
}
.toolbar-title {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.toolbar-title .title {
  font-size: 16px;
  font-weight: 600;
}
.toolbar-title .subtitle {
  font-size: 12px;
  color: var(--el-text-color-secondary);
}
.toolbar-actions {
  display: flex;
  gap: 8px;
}
.filters {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 14px;
}
.filter-search {
  width: 220px;
}
.filter-channel {
  width: 200px;
}
.filter-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--el-text-color-regular);
  font-size: 13px;
}
.filter-toggle-label {
  white-space: nowrap;
}
.summary {
  display: inline-flex;
  gap: 6px;
  margin-left: auto;
}
.channel-card {
  margin-bottom: 12px;
  border: 1px solid #e4e7ed;
}
.channel-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 12px;
}
.channel-name {
  font-weight: bold;
  margin-right: 8px;
}
.model-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: 12px;
  margin-top: 8px;
}
.model-card {
  display: flex;
  flex-direction: column;
  background: #fff;
  border: 1px solid #ebeef5;
  border-radius: 12px;
  padding: 14px 16px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.03);
  transition: box-shadow 0.2s ease, border-color 0.2s ease,
    transform 0.2s ease, background 0.25s ease;
}
.model-card:hover {
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.06);
  transform: translateY(-1px);
}
.model-card.is-enabled {
  border-color: #79bbff;
  background: linear-gradient(135deg, #ecf5ff 0%, #f5fbff 100%);
  box-shadow: 0 2px 8px rgba(64, 158, 255, 0.1);
}
.model-card.is-enabled:hover {
  box-shadow: 0 4px 14px rgba(64, 158, 255, 0.18);
  border-color: #409eff;
}
/* disabled = 基础态的反面：虚线边、淡化背景、灰字、不响应 hover 抬升 */
.model-card.is-disabled {
  background: #fafbfc;
  border-style: dashed;
  border-color: #dcdfe6;
  box-shadow: none;
  opacity: 0.72;
}
.model-card.is-disabled:hover {
  transform: none;
  box-shadow: none;
  border-color: #c0c4cc;
}
.model-card.is-disabled .model-name {
  color: #909399;
}
.model-card.is-disabled .model-card-status {
  background: rgba(0, 0, 0, 0.02);
}
.model-card.is-testing {
  opacity: 0.65;
  pointer-events: none;
}

.model-card-top {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 12px;
  margin-bottom: 12px;
}
.model-name {
  font-weight: 600;
  font-size: 14px;
  color: #1f2329;
  word-break: break-all;
  line-height: 1.5;
  flex: 1;
  min-width: 0;
  letter-spacing: -0.01em;
}
.model-card.is-enabled .model-name {
  color: #0958d9;
}

.model-card-status {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 10px 12px;
  background: rgba(0, 0, 0, 0.025);
  border-radius: 8px;
  margin-bottom: 12px;
  min-height: 38px;
  justify-content: center;
}
.model-card.is-enabled .model-card-status {
  background: rgba(64, 158, 255, 0.08);
}
.status-line {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  font-weight: 500;
  line-height: 18px;
  width: 100%;
}
.status-icon {
  font-size: 14px;
  flex-shrink: 0;
}
.status-line.ok { color: #16a34a; }
.status-line.fail { color: #dc2626; cursor: help; }
.status-proto {
  font-weight: 600;
  text-transform: capitalize;
}
.status-text {
  margin-left: auto;
  font-size: 11px;
  opacity: 0.85;
  letter-spacing: 0.02em;
}
.status-untested {
  color: #909399;
  font-size: 12px;
  font-style: italic;
  text-align: center;
  letter-spacing: 0.04em;
}

.model-card-actions {
  display: flex;
  gap: 8px;
  margin-top: auto;
}
.model-card-actions > .action-btn,
.model-card-actions > .action-pop {
  flex: 1;
  min-width: 0;
}
/* el-popconfirm 包裹了内部按钮，需要让内部按钮填满 popconfirm 宽度 */
.model-card-actions .action-pop {
  display: flex;
}
.model-card-actions .action-pop .action-btn {
  width: 100%;
}
.select-toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 10px;
}
.select-filter {
  margin-bottom: 10px;
}
.select-list {
  max-height: 50vh;
  overflow-y: auto;
  border: 1px solid #e4e7ed;
  border-radius: 6px;
  padding: 6px 10px;
  background: #fafafa;
}
.select-row {
  display: flex;
  width: 100%;
  margin: 0 !important;
  padding: 4px 0;
  word-break: break-all;
}
.select-name {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 13px;
}
</style>