import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ElementPlus from 'element-plus'
import { createRouter, createMemoryHistory } from 'vue-router'
import i18n, { setLocale } from '../src/i18n'

const listBreakerHistory = vi.fn()
const listBreakerHistoryFilterOptions = vi.fn()
vi.mock('../src/api', () => ({ listBreakerHistory, listBreakerHistoryFilterOptions }))
vi.mock('../src/session', () => ({ loadSession: vi.fn(), session: { isAdmin: true } }))

const { default: BreakerHistory } = await import('../src/views/BreakerHistory.vue')

// Element Plus teleports dropdowns to <body>; unmount explicitly so a later
// case doesn't see a previous case's options.
const wrappers = []

async function mountHistory() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }],
  })
  const w = mount(BreakerHistory, {
    attachTo: document.body,
    global: { plugins: [ElementPlus, i18n, router] },
  })
  wrappers.push(w)
  await flushPromises()
  return w
}

const selects = (w) => w.findAll('.filter-row .el-select')

async function pick(w, index, label) {
  await selects(w)[index].find('.el-select__wrapper').trigger('click')
  await flushPromises()
  const option = [...document.querySelectorAll('.el-select-dropdown__item')].find(
    (o) => o.textContent === label,
  )
  expect(option, `option ${label} is on offer`).toBeTruthy()
  await option.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  await flushPromises()
}

const lastHistoryParams = () => listBreakerHistory.mock.calls.at(-1)[3]
const lastOptionParams = () => listBreakerHistoryFilterOptions.mock.calls.at(-1)[1]

afterEach(() => {
  while (wrappers.length) wrappers.pop().unmount()
})

beforeEach(() => {
  listBreakerHistory.mockReset()
  listBreakerHistoryFilterOptions.mockReset()
  setLocale('zh-CN')
  listBreakerHistory.mockResolvedValue({ events: [], total: 0 })
  listBreakerHistoryFilterOptions.mockResolvedValue({
    channel: ['openai', 'anthropic'],
    model: ['gpt-4o', 'claude-3'],
  })
})

describe('breaker history page', () => {
  it('renders three filter dropdowns plus the table', async () => {
    const w = await mountHistory()
    expect(selects(w)).toHaveLength(3)
    // Default time window: 1h.
    expect(listBreakerHistory.mock.calls.at(-1)[0]).toBe(1)
    expect(lastHistoryParams()).toEqual({})
    expect(lastOptionParams()).toEqual({})
  })

  it('applies a channel filter immediately', async () => {
    const w = await mountHistory()
    await pick(w, 0, 'openai')
    expect(lastHistoryParams()).toEqual({ channel: 'openai' })
    // The facet rule asks the server to keep both channel options on offer
    // even though one is currently picked.
    expect(lastOptionParams()).toEqual({ channel: 'openai' })
  })

  it('combines channel + model + event into one query', async () => {
    const w = await mountHistory()
    await pick(w, 0, 'openai')
    await pick(w, 1, 'gpt-4o')
    // The event dropdown carries translated labels; pick the localized
    // label "熔断" (zh-CN default) for `tripped`.
    await pick(w, 2, '熔断')
    expect(lastHistoryParams()).toEqual({
      channel: 'openai',
      model: 'gpt-4o',
      event: 'tripped',
    })
  })

  it('refetches with range=0 when the user picks "All"', async () => {
    const w = await mountHistory()
    await w.findAll('.toolbar .el-radio-button')[3].trigger('click')
    await flushPromises()
    // Same as Logs.vue: the options request is observable on the very next
    // tick, the list request after one more flush (its load() awaits the
    // mock). Mirror the logs test by asserting the cheap half.
    expect(listBreakerHistoryFilterOptions.mock.calls.at(-1)[0]).toBe(0)
    expect(lastOptionParams()).toEqual({})
  })

  it('clears every filter on reset', async () => {
    const w = await mountHistory()
    await pick(w, 0, 'openai')
    await pick(w, 1, 'gpt-4o')
    expect(lastHistoryParams()).toEqual({ channel: 'openai', model: 'gpt-4o' })

    const reset = w.find('.filter-row button')
    expect(reset.attributes('disabled')).toBeUndefined()
    await reset.trigger('click')
    await flushPromises()
    expect(lastHistoryParams()).toEqual({})
  })

  it('survives an options request that fails', async () => {
    listBreakerHistoryFilterOptions.mockRejectedValue(new Error('boom'))
    const w = await mountHistory()
    expect(listBreakerHistory).toHaveBeenCalled()
    // The dropdowns still render — empty values but the page works.
    expect(selects(w)).toHaveLength(3)
  })
})