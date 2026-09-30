<template>
  <el-card v-loading="loading">
    <div class="toolbar">
      <span>{{ channelId ? `配置渠道「${channelName}」的模型` : '从上游获取模型列表后勾选该渠道对外提供的模型' }}</span>
      <div>
        <el-button v-if="channelId" @click="back">返回渠道列表</el-button>
        <el-button v-else @click="load">刷新</el-button>
      </div>
    </div>

    <el-empty v-if="!channels.length && !loading" description="还没有渠道，请先在渠道管理中添加" />

    <el-card v-for="ch in channels" :key="ch.id" class="channel-card" shadow="never">
      <div class="channel-head">
        <div>
          <span class="channel-name">{{ ch.name }}</span>
          <el-tag v-if="ch.enabled" type="success" size="small">启用</el-tag>
          <el-tag v-else type="danger" size="small">禁用</el-tag>
          <el-tag v-if="ch.base_url" size="small">OpenAI</el-tag>
          <el-tag v-if="ch.base_url_anthropic" size="small">Anthropic</el-tag>
        </div>
        <div>
          <el-button size="small" :loading="ch._testingAll"
            :disabled="!modelList(ch).length" @click="testAll(ch)">
            全部测试
          </el-button>
          <el-button size="small" :loading="ch._fetching"
            :disabled="!ch.base_url && !ch.base_url_anthropic" @click="fetchList(ch)">
            获取模型列表
          </el-button>
          <el-button size="small" @click="openManual(ch)">手动添加</el-button>
        </div>
      </div>

      <div v-if="!ch.base_url" class="hint">
        该渠道只配置了 Anthropic 协议，暂不支持自动获取（可手动添加模型）
      </div>

      <!-- 模型卡片网格 -->
      <div v-if="modelList(ch).length" class="model-grid">
        <div v-for="m in modelList(ch)" :key="m" class="model-card"
             :class="{ 'is-enabled': isSelected(ch, m), 'is-testing': ch._testingModel === m }">
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
                            :content="ch._testResult[m].protocols[proto].error || '不可用'"
                            placement="top">
                  <div class="status-line fail">
                    <el-icon class="status-icon"><CircleCloseFilled /></el-icon>
                    <span class="status-proto">{{ proto }}</span>
                    <span class="status-text">不可用</span>
                  </div>
                </el-tooltip>
                <div v-else class="status-line ok">
                  <el-icon class="status-icon"><CircleCheckFilled /></el-icon>
                  <span class="status-proto">{{ proto }}</span>
                  <span class="status-text">可用</span>
                </div>
              </template>
            </template>
            <div v-if="!hasTestResult(ch, m)" class="status-untested">尚未测试</div>
          </div>
          <div class="model-card-actions">
            <el-button class="action-btn" :loading="ch._testingModel === m"
                       @click="pingModel(ch, m)">
              <el-icon><Refresh /></el-icon>
              <span>测试</span>
            </el-button>
            <el-popconfirm class="action-pop"
                           :title="`从「${ch.name}」移除「${m}」？该模型的测试结果也会清空`"
                           confirm-button-text="移除"
                           cancel-button-text="取消"
                           @confirm="removeModel(ch, m)">
              <template #reference>
                <el-button class="action-btn" type="danger" plain>
                  <el-icon><Delete /></el-icon>
                  <span>删除</span>
                </el-button>
              </template>
            </el-popconfirm>
          </div>
        </div>
      </div>
      <span v-else class="hint">尚未配置模型</span>
    </el-card>

    <!-- 手动添加模型 -->
    <el-dialog v-model="manualVisible" title="手动添加模型" width="480px">
      <p class="hint" style="margin-top: 0">逗号分隔，追加到该渠道的模型列表</p>
      <el-input v-model="manualInput" type="textarea" :rows="4" placeholder="如 gpt-4o,gpt-4o-mini,gpt-3.5-turbo" />
      <template #footer>
        <el-button @click="manualVisible = false">取消</el-button>
        <el-button type="primary" @click="saveManual">保存</el-button>
      </template>
    </el-dialog>

    <!-- 选择要添加的模型 -->
    <el-dialog v-model="selectVisible"
      :title="`选择要添加到「${selectTarget?.name || ''}」的模型`"
      width="560px" top="6vh" @closed="onSelectClosed">
      <div class="select-toolbar">
        <span class="hint">已选 {{ selectedArr.length }} / {{ fetchedList.length }}</span>
        <div>
          <el-button size="small" @click="selectAll">全选</el-button>
          <el-button size="small" @click="selectNone">全不选</el-button>
          <el-button size="small" @click="selectInverse">反选</el-button>
        </div>
      </div>
      <el-input v-model="selectFilter" placeholder="过滤模型名（不区分大小写）"
        clearable class="select-filter" />
      <div class="select-list">
        <el-checkbox-group v-model="selectedArr">
          <el-checkbox v-for="m in filteredList" :key="m" :value="m" class="select-row">
            <span class="select-name">{{ m }}</span>
          </el-checkbox>
        </el-checkbox-group>
        <el-empty v-if="!filteredList.length" description="无匹配模型" :image-size="60" />
      </div>
      <template #footer>
        <el-button @click="selectVisible = false">取消</el-button>
        <el-button type="primary" :loading="selectSaving" @click="confirmSelect">
          添加到渠道
        </el-button>
      </template>
    </el-dialog>
  </el-card>
</template>

<script setup>
import { computed, onMounted, reactive, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { CircleCheckFilled, CircleCloseFilled, Delete, Refresh } from '@element-plus/icons-vue'
import { listChannels, updateChannelModels, fetchModels, testModel } from '../api'

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
// known[id] = 该渠道出现过的所有模型名（已配置 + 上游拉取 + 手动添加的并集）
// 切换启用/停用只改 ch.models，不改 known——保证停用的模型不会从卡片列表里消失
const known = reactive({})

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

function splitModels(s) {
  return (s || '').split(',').map((x) => x.trim()).filter((x) => x && x !== '*')
}

const PROTOCOLS = ['openai', 'anthropic']

function hasTestResult(ch, m) {
  const r = ch._testResult?.[m]
  return r && r.protocols && Object.keys(r.protocols).length > 0
}

function isSelected(ch, m) {
  return splitModels(ch.models).includes(m)
}

async function load() {
  loading.value = true
  try {
    const all = await listChannels()
    channels.value = channelId.value ? all.filter((c) => c.id === channelId.value) : all
    // seed known list from current config so toggling-off never hides a card
    for (const ch of channels.value) rememberModels(ch.id, splitModels(ch.models))
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
    })
    if (!models.length) {
      ElMessage.warning('上游未返回任何模型')
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
  const merged = [...new Set([...oldModels, ...selectedArr.value])]
  const newModels = merged.join(',')
  // 没变化就不写库，但弹个提示让用户知道发生了什么
  if (newModels === ch.models) {
    selectVisible.value = false
    ElMessage.info('没有变化，未保存')
    return
  }
  selectSaving.value = true
  try {
    await updateChannelModels(ch.id, newModels)
    ch.models = newModels
    // 只把"真正要在这个渠道里"的模型加入 known——上游拉到的全部模型只
    // 是候选，未勾选的就不该出现卡片，否则刷新后也没了。
    // rememberModels 自身去重，selectedArr 里已存在的旧模型是 no-op。
    rememberModels(ch.id, selectedArr.value)
    const oldSet = new Set(oldModels)
    const added = selectedArr.value.filter((m) => !oldSet.has(m)).length
    ElMessage.success(
      added > 0
        ? `已添加 ${added} 个模型到「${ch.name}」`
        : `「${ch.name}」模型列表已更新`,
    )
    selectVisible.value = false
  } catch (e) {
    const detail = e?.response?.data?.message || e.message || '未知错误'
    ElMessage.error(`保存失败：${detail}`)
  } finally {
    selectSaving.value = false
  }
}

async function toggleModel(ch, m, enabled) {
  const set = new Set(splitModels(ch.models))
  if (enabled) set.add(m)
  else set.delete(m)
  const newModels = [...set].join(',')
  if (newModels === ch.models) return
  ch._togglingModel = m
  try {
    await updateChannelModels(ch.id, newModels)
    ch.models = newModels
  } catch (e) {
    ElMessage.error(`更新 ${m} 失败`)
  } finally {
    ch._togglingModel = null
  }
}

// 完全删除：从 ch.models 移除并持久化，从 known 移除（卡片消失），
// 测试结果也清掉。要恢复只能重新获取或手动添加。
async function removeModel(ch, m) {
  // 1. 持久化 ch.models（如果该模型在其中）
  const cur = splitModels(ch.models)
  const inSaved = cur.includes(m)
  if (inSaved) {
    const newModels = cur.filter((x) => x !== m).join(',')
    try {
      await updateChannelModels(ch.id, newModels)
      ch.models = newModels
    } catch (e) {
      const detail = e?.response?.data?.message || e.message || '未知错误'
      ElMessage.error(`移除 ${m} 失败：${detail}`)
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
  ElMessage.success(`已从「${ch.name}」移除 ${m}`)
}

async function pingModel(ch, m) {
  ch._testingModel = m
  try {
    const r = await testModel({
      base_url: ch.base_url,
      base_url_anthropic: ch.base_url_anthropic,
      api_key: ch.api_key,
      model: m,
    })
    if (!ch._testResult) ch._testResult = {}
    ch._testResult[m] = r
    // toast summary
    const ok = PROTOCOLS.filter((p) => r.protocols?.[p]?.ok)
    if (r.ok) ElMessage.success(`${m}: ${ok.join(' + ') || '可用'}`)
    else ElMessage.error(`${m}: 不可用`)
  } catch (e) {
    if (!ch._testResult) ch._testResult = {}
    ch._testResult[m] = { ok: false, protocols: {} }
    ElMessage.error(`${m}: 测试失败`)
  } finally {
    ch._testingModel = null
  }
}

async function testAll(ch) {
  const list = modelList(ch)
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
    if (fail === 0) ElMessage.success(`${ch.name}：${ok} 个模型全部可用`)
    else ElMessage.warning(`${ch.name}：${ok} 个可用，${fail} 个不可用`)
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
    ElMessage.warning('请输入至少一个模型名')
    return
  }
  const models = [...new Set([...splitModels(ch.models), ...adds])].join(',')
  await updateChannelModels(ch.id, models)
  ch.models = models
  rememberModels(ch.id, adds)
  manualVisible.value = false
  ElMessage.success('已添加')
}

onMounted(load)
</script>

<style scoped>
.toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 12px;
  color: #606266;
  font-size: 13px;
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
.hint {
  color: #909399;
  font-size: 12px;
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