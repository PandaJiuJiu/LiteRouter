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
             :class="{ 'is-enabled': isSelected(ch, m), testing: ch._testingModel === m }">
          <div class="model-card-header">
            <span class="model-name" :title="m">{{ m }}</span>
            <el-switch :model-value="isSelected(ch, m)"
                       :loading="ch._togglingModel === m"
                       @change="(v) => toggleModel(ch, m, v)" />
          </div>
          <div class="model-card-footer">
            <div class="model-status">
              <template v-for="proto in PROTOCOLS" :key="proto">
                <template v-if="ch._testResult?.[m]?.protocols?.[proto]">
                  <el-tooltip v-if="ch._testResult[m].protocols[proto].ok === false"
                              :content="ch._testResult[m].protocols[proto].error || '不可用'"
                              placement="top">
                    <span class="status-pill fail">
                      <el-icon><CircleCloseFilled /></el-icon>
                      {{ proto }}
                    </span>
                  </el-tooltip>
                  <span v-else class="status-pill ok">
                    <el-icon><CircleCheckFilled /></el-icon>
                    {{ proto }}
                  </span>
                </template>
              </template>
              <span v-if="!hasTestResult(ch, m)" class="untested">未测试</span>
            </div>
            <el-button size="small" :loading="ch._testingModel === m"
                       @click="pingModel(ch, m)">
              <el-icon><Refresh /></el-icon>
              <span>测试</span>
            </el-button>
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
  </el-card>
</template>

<script setup>
import { computed, onMounted, reactive, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { CircleCheckFilled, CircleCloseFilled, Refresh } from '@element-plus/icons-vue'
import { listChannels, updateChannel, fetchModels, testModel } from '../api'

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

function savePayload(ch, models) {
  return {
    name: ch.name,
    base_url: ch.base_url,
    base_url_anthropic: ch.base_url_anthropic || '',
    api_key: ch.api_key,
    enabled: !!ch.enabled,
    models,
  }
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
    const added = rememberModels(ch.id, models)
    ElMessage.success(
      added > 0
        ? `获取到 ${models.length} 个模型，新增 ${added} 个`
        : `获取到 ${models.length} 个模型（无新增）`,
    )
  } finally {
    ch._fetching = false
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
    await updateChannel(ch.id, savePayload(ch, newModels))
    ch.models = newModels
  } catch (e) {
    ElMessage.error(`更新 ${m} 失败`)
  } finally {
    ch._togglingModel = null
  }
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
  await updateChannel(ch.id, savePayload(ch, models))
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
  border: 1px solid #e4e7ed;
  border-radius: 8px;
  padding: 12px 14px;
  background: #fff;
  transition: border-color 0.15s ease, background 0.15s ease;
}
.model-card.is-enabled {
  border-color: #409eff;
  background: #ecf5ff;
}
.model-card.testing {
  opacity: 0.75;
}
.model-card-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 12px;
  margin-bottom: 10px;
}
.model-name {
  font-weight: 600;
  font-size: 14px;
  color: #303133;
  word-break: break-all;
  line-height: 1.4;
  flex: 1;
  min-width: 0;
}
.model-card.is-enabled .model-name {
  color: #409eff;
}
.model-card-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  border-top: 1px dashed #e4e7ed;
  padding-top: 8px;
}
.model-status {
  font-size: 12px;
  color: #606266;
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
}
.model-status .ok { color: #67c23a; font-size: 14px; }
.model-status .fail { color: #f56c6c; font-size: 14px; }
.model-status .via {
  color: #909399;
  margin-left: 2px;
}
.status-pill {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  padding: 1px 6px;
  border-radius: 10px;
  font-size: 11px;
  font-weight: 500;
  line-height: 16px;
  cursor: default;
}
.status-pill.ok {
  background: #f0f9eb;
  color: #67c23a;
}
.status-pill.fail {
  background: #fef0f0;
  color: #f56c6c;
  cursor: help;
}
.status-pill .el-icon {
  font-size: 12px;
}
.fail-text {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  cursor: help;
}
.untested {
  color: #c0c4cc;
  font-style: italic;
}
.hint {
  color: #909399;
  font-size: 12px;
}
</style>