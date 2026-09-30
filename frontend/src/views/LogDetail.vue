<template>
  <el-card v-loading="loading">
    <div class="header">
      <el-button text @click="back">‹ 返回日志列表</el-button>
      <span class="title">调用详情 #{{ log?.id }}</span>
    </div>

    <template v-if="log">
      <!-- 一眼扫到的状态条 -->
      <div class="status-bar">
        <el-tag :type="ok ? 'success' : 'danger'" size="large">{{ log.status_code }}</el-tag>
        <span class="latency" v-if="log.latency_ms > 0">
          {{ log.latency_ms.toLocaleString() }} ms
          <span class="hint">（上游响应耗时）</span>
        </span>
        <el-tag v-if="log.stream" size="small" type="info">stream</el-tag>
        <el-tag v-if="log.convert && log.convert !== 'none'" size="small" type="warning">
          协议转换 {{ log.convert }}
        </el-tag>
      </div>

      <el-alert v-if="log.error" type="error" :title="log.error" :closable="false" show-icon />

      <!-- 基本信息 -->
      <section>
        <h3>请求</h3>
        <el-descriptions :column="2" border>
          <el-descriptions-item label="令牌">{{ log.token_name }}</el-descriptions-item>
          <el-descriptions-item label="渠道">{{ log.channel_name }}</el-descriptions-item>
          <el-descriptions-item label="客户端协议">{{ log.protocol || '—' }}</el-descriptions-item>
          <el-descriptions-item label="时间">{{ formatTime(log.created_at) }}</el-descriptions-item>
          <el-descriptions-item label="客户端请求模型">{{ log.request_model || '—' }}</el-descriptions-item>
          <el-descriptions-item label="上游实际模型">{{ log.upstream_model || '—' }}</el-descriptions-item>
        </el-descriptions>
      </section>

      <!-- Token 明细：cache 段只在该次请求非零时显示 -->
      <section>
        <h3>Token 消耗</h3>
        <el-descriptions :column="1" border>
          <el-descriptions-item label="合计">{{ log.total_tokens.toLocaleString() }}</el-descriptions-item>
          <el-descriptions-item label="输入（prompt / input）">{{ log.prompt_tokens.toLocaleString() }}</el-descriptions-item>
          <el-descriptions-item label="输出（completion / output）">{{ log.completion_tokens.toLocaleString() }}</el-descriptions-item>
          <el-descriptions-item v-if="log.cache_read_tokens > 0" label="缓存命中（cache_read）">
            {{ log.cache_read_tokens.toLocaleString() }}
            <span class="hint">—— 按 ~0.1 倍计费</span>
          </el-descriptions-item>
          <el-descriptions-item v-if="log.cache_creation_tokens > 0" label="缓存写入（cache_creation）">
            {{ log.cache_creation_tokens.toLocaleString() }}
            <span class="hint">—— 按 ~1.25 倍计费</span>
          </el-descriptions-item>
          <el-descriptions-item v-if="log.reasoning_tokens > 0" label="推理（reasoning）">
            {{ log.reasoning_tokens.toLocaleString() }}
            <span class="hint">—— 已含在输出中</span>
          </el-descriptions-item>
        </el-descriptions>
      </section>
    </template>
  </el-card>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getLog } from '../api'

const route = useRoute()
const router = useRouter()
const log = ref(null)
const loading = ref(false)
const ok = ref(false)

async function load() {
  loading.value = true
  try {
    log.value = await getLog(route.params.id)
    ok.value = log.value && log.value.status_code >= 200 && log.value.status_code < 300
  } finally {
    loading.value = false
  }
}

function formatTime(ts) {
  return new Date(ts * 1000).toLocaleString()
}

function back() {
  router.push('/logs')
}

onMounted(load)
</script>

<style scoped>
.header {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 16px;
}
.title {
  color: #606266;
  font-size: 13px;
}
.status-bar {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 12px;
}
.latency {
  font-variant-numeric: tabular-nums;
}
.hint {
  color: #909399;
  font-size: 12px;
}
section {
  margin-top: 20px;
}
h3 {
  margin: 0 0 8px;
  font-size: 14px;
  color: #303133;
}
</style>