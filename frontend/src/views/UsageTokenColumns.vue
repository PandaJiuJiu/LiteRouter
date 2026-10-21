<template>
  <!-- The four usage tabs differ only in their leading column; the numeric
       columns below were copy-pasted into each table by hand, so a label or
       width change meant four edits and three misses. -->
  <el-table-column prop="requests" :label="t('usage.col.requests')" width="120" sortable />
  <el-table-column prop="prompt_tokens" :label="t('usage.col.promptTokens')" width="140" sortable :formatter="tableNum" />
  <el-table-column prop="completion_tokens" :label="t('usage.col.completionTokens')" width="140" sortable :formatter="tableNum" />
  <el-table-column prop="total_tokens" :label="t('usage.col.totalTokens')" width="140" sortable :formatter="tableNum" />
  <el-table-column prop="cache_read_tokens" :label="t('usage.col.cacheReadTokens')" width="140" sortable :formatter="tableNum" />
  <el-table-column prop="cache_hit_rate" :label="t('usage.col.cacheHitRate')" width="130" sortable :formatter="ratePct" />
</template>

<script setup>
import { useI18n } from 'vue-i18n'
import { tableNum } from '../format'

const { t } = useI18n()

// The backend sends the rate as a percentage with one decimal. A row with
// no prompt at all has no rate, and the backend sends 0 for it.
const ratePct = (_row, _column, value) => `${(value ?? 0).toFixed(1)}%`
</script>