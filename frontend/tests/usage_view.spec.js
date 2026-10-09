import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ElementPlus from 'element-plus'
import { createRouter, createMemoryHistory } from 'vue-router'
import i18n, { setLocale } from '../src/i18n'
import zhCN from '../src/i18n/locales/zh-CN'

const fetchUsage = vi.fn()
const me = vi.fn()
vi.mock('../src/api', () => ({ fetchUsage, me }))

const { default: Usage } = await import('../src/views/Usage.vue')

const ROWS = {
  by_token: [{ key: 'sk-aaaa', requests: 3, prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 }],
  by_model: [{ key: 'gpt-4o', requests: 3, prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 }],
  by_channel: [{ key: 'ark', requests: 3, prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 }],
  by_day: [{ key: '2026-10-01', day: 1_788_000_000, requests: 3, prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 }],
}

async function mountUsage() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }],
  })
  const w = mount(Usage, { global: { plugins: [ElementPlus, i18n, router] } })
  await flushPromises()
  return w
}

/** Header labels of the Nth el-table, in render order. */
const headers = (w, n) =>
  w.findAll('.el-table__header-wrapper')[n].findAll('th').map((h) => h.text())

beforeEach(() => {
  fetchUsage.mockReset()
  me.mockReset()
  setLocale('zh-CN')
  me.mockResolvedValue({ username: 'ada', is_admin: false })
  fetchUsage.mockResolvedValue(ROWS)
})

describe('usage tables', () => {
  it('gives every table a leading column plus the four shared numeric ones', async () => {
    // The four tabs used to carry hand-copied copies of the numeric columns; a
    // label or width change meant four edits and three misses.
    const w = await mountUsage()
    const shared = [
      zhCN.usage.col.requests,
      zhCN.usage.col.promptTokens,
      zhCN.usage.col.completionTokens,
      zhCN.usage.col.totalTokens,
    ]
    for (let n = 0; n < 4; n++) {
      expect(headers(w, n).slice(1)).toEqual(shared)
    }
    expect(headers(w, 0)[0]).toBe(zhCN.usage.col.internalToken)
    expect(headers(w, 1)[0]).toBe(zhCN.usage.col.model)
    expect(headers(w, 2)[0]).toBe(zhCN.usage.col.channel)
    expect(headers(w, 3)[0]).toBe(zhCN.usage.col.date)
  })

  it('formats the shared numeric columns with thousands separators', async () => {
    const w = await mountUsage()
    w.vm.by_token = [{ key: 'sk-aaaa', requests: 1, prompt_tokens: 1_234_567, completion_tokens: 0, total_tokens: 1_234_567 }]
    await flushPromises()
    const cells = w.findAll('.el-table__body-wrapper')[0].findAll('td').map((c) => c.text())
    expect(cells[2]).toBe((1_234_567).toLocaleString())
  })

  it('drops the nameless all-failed bucket from the channel tab only', async () => {
    // The backend logs a request whose every candidate failed with a blank
    // channel_name — deliberately, rather than blaming the last channel
    // tried. Those group into an empty-key bucket that is not a channel, so
    // the channel tab filters it. It must still be counted everywhere else.
    fetchUsage.mockResolvedValue({
      ...ROWS,
      totals: { requests: 7, prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 },
      by_channel: [
        { key: 'ark', requests: 3, prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 },
        { key: '', requests: 4, prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
      ],
    })
    const w = await mountUsage()
    expect(w.vm.by_channel.map((r) => r.key)).toEqual(['ark'])
    // Not filtered out of the other tabs, and still in the totals.
    expect(w.vm.by_token).toHaveLength(1)
    expect(w.vm.totes.requests).toBe(7)
  })
})

describe('usage bar chart', () => {
  const barValues = (w) => w.findAll('.bar').map((b) => b.attributes('data-value'))

  it('plots the active tab, one bar per row, defaulting to total tokens', async () => {
    fetchUsage.mockResolvedValue({
      ...ROWS,
      by_token: [
        { key: 'a', requests: 1, prompt_tokens: 1, completion_tokens: 2, total_tokens: 3 },
        { key: 'b', requests: 4, prompt_tokens: 5, completion_tokens: 6, total_tokens: 11 },
      ],
    })
    const w = await mountUsage()
    expect(barValues(w)).toEqual(['3', '11'])
  })

  it('follows the selected tab', async () => {
    // by_token has two rows, by_model one: switching tabs must swap the bars,
    // not leave the previous dimension on screen.
    fetchUsage.mockResolvedValue({
      ...ROWS,
      by_token: [
        { key: 'a', requests: 1, prompt_tokens: 1, completion_tokens: 2, total_tokens: 3 },
        { key: 'b', requests: 4, prompt_tokens: 5, completion_tokens: 6, total_tokens: 11 },
      ],
      by_model: [{ key: 'gpt-4o', requests: 2, prompt_tokens: 0, completion_tokens: 0, total_tokens: 7 }],
    })
    const w = await mountUsage()
    expect(w.findAll('.bar')).toHaveLength(2)
    w.vm.activeTab = 'model'
    await flushPromises()
    expect(barValues(w)).toEqual(['7'])
  })

  it('re-plots the same rows when the metric selector changes', async () => {
    fetchUsage.mockResolvedValue({
      ...ROWS,
      by_token: [{ key: 'a', requests: 2, prompt_tokens: 7, completion_tokens: 5, total_tokens: 15 }],
    })
    const w = await mountUsage()
    expect(barValues(w)).toEqual(['15'])
    w.vm.metric = 'prompt_tokens'
    await flushPromises()
    expect(barValues(w)).toEqual(['7'])
  })

  it('shows the empty hint when the active tab has no usage', async () => {
    fetchUsage.mockResolvedValue({ by_token: [], by_model: [], by_channel: [], by_day: [] })
    const w = await mountUsage()
    expect(w.find('.empty').text()).toBe(zhCN.usage.chart.empty)
  })
})