<template>
  <div ref="wrap" class="bar-chart">
    <svg
      :width="chartWidth"
      :height="HEIGHT"
      :viewBox="`0 0 ${chartWidth} ${HEIGHT}`"
      role="img"
      :aria-label="t('usage.chart.title')"
      @mouseleave="hover = null"
    >
      <!-- Horizontal gridlines, one per tick, with its value on the y-axis. -->
      <g v-for="(tick, ti) in ticks" :key="`g${ti}`">
        <line
          :x1="PAD.left"
          :x2="chartWidth - PAD.right"
          :y1="yScale(tick)"
          :y2="yScale(tick)"
          :class="['grid', { base: tick === 0 }]"
        />
        <text
          :x="PAD.left - 10"
          :y="yScale(tick) + 4"
          class="y-label"
          text-anchor="end"
        >{{ fmtCompact(tick) }}</text>
      </g>

      <!-- One bar per row; the rect carries its value so the chart is
           assertable without reaching into SVG geometry. -->
      <rect
        v-for="(row, i) in rows"
        :key="`b${i}`"
        class="bar"
        :class="{ active: hover === i }"
        :x="barX(i)"
        :y="yScale(valueOf(row))"
        :width="barW"
        :height="barHeight(row)"
        :data-value="valueOf(row)"
        rx="3"
        @mouseenter="hover = i"
      />

      <text
        v-if="!hasData"
        class="empty"
        :x="chartWidth / 2"
        :y="HEIGHT / 2"
        text-anchor="middle"
      >{{ t('usage.chart.empty') }}</text>
    </svg>

    <div v-if="hover !== null && hasData" class="tip" :style="tipStyle">
      <div class="tip-key">{{ labelText(rows[hover]) }}</div>
      <div class="tip-val">{{ metricLabel }}</div>
      <div class="tip-num">{{ fmtNum(valueOf(rows[hover])) }}</div>
    </div>
  </div>
</template>

<script setup>
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { fmtCompact, fmtDate, fmtNum } from '../format'

const props = defineProps({
  /** The rows of the currently active table. */
  rows: { type: Array, default: () => [] },
  /** Which numeric field to plot (requests / prompt_tokens / …). */
  valueKey: { type: String, default: 'total_tokens' },
  /** Which field labels a bar (`key`, or `day`). */
  labelKey: { type: String, default: 'key' },
  /** Render the label as `YYYY-MM-DD` instead of the raw key. */
  isDay: { type: Boolean, default: false },
  /** Human label for the plotted metric, shown in the tooltip. */
  metricLabel: { type: String, default: '' },
})

const { t } = useI18n()

// Height is fixed; width tracks the container so the chart fills the card on
// any screen. `PAD` leaves room for the y-axis numbers; the x-axis carries no
// labels (the tooltip names each bar), so the bottom just needs breathing room.
const HEIGHT = 300
const PAD = { top: 16, right: 16, bottom: 22, left: 56 }
const plotH = HEIGHT - PAD.top - PAD.bottom
const plotBottom = HEIGHT - PAD.bottom

const wrap = ref(null)
const width = ref(0)
let observer = null

onMounted(() => {
  if (wrap.value) width.value = wrap.value.clientWidth || 0
  // jsdom (the test env) has no ResizeObserver; the 640px fallback below keeps
  // the component renderable there.
  if (typeof ResizeObserver !== 'undefined' && wrap.value) {
    observer = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect?.width
      if (w) width.value = w
    })
    observer.observe(wrap.value)
  }
})
onBeforeUnmount(() => observer?.disconnect())

const chartWidth = computed(() => (width.value > 0 ? width.value : 640))

function valueOf(row) {
  const v = Number(row?.[props.valueKey])
  return Number.isFinite(v) && v > 0 ? v : 0
}

/** Round tick steps (1/2/5×10ⁿ) so the y-axis reads cleanly. */
function niceTicks(max, count) {
  if (!(max > 0)) return [0, 1]
  const raw = max / count
  const mag = 10 ** Math.floor(Math.log10(raw))
  const norm = raw / mag
  const step = (norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10) * mag
  const top = Math.ceil(max / step) * step
  const out = []
  for (let v = 0; v <= top + step / 2; v += step) out.push(Math.round(v))
  return out
}

const ticks = computed(() => niceTicks(Math.max(0, ...props.rows.map(valueOf)), 4))
const yMax = computed(() => ticks.value[ticks.value.length - 1] || 1)
const yScale = (v) => plotBottom - (v / yMax.value) * plotH

const slotW = computed(() => (props.rows.length ? (chartWidth.value - PAD.left - PAD.right) / props.rows.length : 0))
const barW = computed(() => Math.max(4, Math.min(slotW.value * 0.6, 48)))
const barX = (i) => PAD.left + slotW.value * i + (slotW.value - barW.value) / 2
const barCenter = (i) => PAD.left + slotW.value * i + slotW.value / 2
const barHeight = (row) => Math.max(0, plotBottom - yScale(valueOf(row)))

// The tooltip names the hovered bar; the x-axis itself is unlabelled so a busy
// dimension doesn't turn into a wall of overlapping rotated text.
const labelText = (row) => {
  const raw = props.isDay ? fmtDate(row?.day) : String(row?.[props.labelKey] ?? '')
  return raw.length > 18 ? `${raw.slice(0, 17)}…` : raw
}

const hasData = computed(() => props.rows.length > 0 && Math.max(0, ...props.rows.map(valueOf)) > 0)

const hover = ref(null)
const tipStyle = computed(() => {
  if (hover.value === null) return {}
  const row = props.rows[hover.value]
  return { left: `${barCenter(hover.value)}px`, top: `${yScale(valueOf(row)) - 10}px` }
})
</script>

<style scoped>
.bar-chart {
  position: relative;
  width: 100%;
}
.bar-chart svg {
  display: block;
  overflow: visible;
}
.grid {
  stroke: #ebeef5;
  stroke-width: 1;
}
.grid.base {
  stroke: #dcdfe6;
}
.y-label {
  fill: #909399;
  font-size: 11px;
}
.bar {
  fill: #a0cfff;
  transition: fill 0.15s ease;
}
.bar.active {
  fill: #409eff;
}
.empty {
  fill: #c0c4cc;
  font-size: 13px;
}
.tip {
  position: absolute;
  transform: translate(-50%, -100%);
  padding: 6px 10px;
  border-radius: 6px;
  background: #303133;
  color: #fff;
  font-size: 12px;
  line-height: 1.4;
  white-space: nowrap;
  pointer-events: none;
  box-shadow: 0 2px 8px rgb(0 0 0 / 20%);
  z-index: 5;
}
.tip-val {
  margin-top: 2px;
  color: #c0c4cc;
  font-size: 11px;
}
.tip-num {
  font-weight: 600;
}
</style>
