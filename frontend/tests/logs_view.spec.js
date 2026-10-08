import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ElementPlus from 'element-plus'
import { createRouter, createMemoryHistory } from 'vue-router'
import i18n, { setLocale } from '../src/i18n'

const listLogs = vi.fn()
const listLogFilterOptions = vi.fn()
const loadDebugLogging = vi.fn()
vi.mock('../src/api', () => ({ listLogs, listLogFilterOptions }))
vi.mock('../src/session', () => ({ loadSession: vi.fn(), session: { isAdmin: false } }))
vi.mock('../src/debug', () => ({
  debugLogging: { enabled: false, loading: false },
  loadDebugLogging,
  toggleDebugLogging: vi.fn(),
}))

const { default: Logs } = await import('../src/views/Logs.vue')

const OPTIONS = {
  ip: ['10.0.0.1', '10.0.0.2'],
  token: ['bob-token', 'carol-token'],
  model: ['gpt-4o', 'my-gpt4o'],
  upstream_model: ['gpt-4o', 'gpt-4o-2024'],
  status: [200, 429],
}

// Element Plus teleports dropdowns to <body> and never takes them down on its
// own in jsdom (the leave transition never runs), so every wrapper is unmounted
// explicitly — otherwise a later case finds a previous case's options.
const wrappers = []

async function mountLogs() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }],
  })
  // attachTo puts the wrapper in the document, so Element Plus can measure it
  // and teleport its dropdown where a real browser would.
  const w = mount(Logs, { attachTo: document.body, global: { plugins: [ElementPlus, i18n, router] } })
  wrappers.push(w)
  await flushPromises()
  return w
}

/** The five dropdowns, in template order. */
const selects = (w) => w.findAll('.filter-row .el-select')

/**
 * Pick a value in one of the dropdowns the way a user does: open it, click the
 * matching option, let the close transition land. Element Plus only emits
 * `change` on a real selection, so setting `v-model` directly would skip the
 * very thing these tests are about.
 */
async function pick(w, index, label) {
  // Element Plus teleports each select's dropdown to <body>, so the options are
  // not inside the wrapper — query the document. Only the open select renders
  // its items, so this can't pick up a neighbour's list.
  await selects(w)[index].find('.el-select__wrapper').trigger('click')
  await flushPromises()
  const option = [...document.querySelectorAll('.el-select-dropdown__item')].find(
    (o) => o.textContent === label
  )
  expect(option, `option ${label} is on offer`).toBeTruthy()
  await option.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  await flushPromises()
}

const lastLogParams = () => listLogs.mock.calls.at(-1)[3]
const lastOptionParams = () => listLogFilterOptions.mock.calls.at(-1)[1]

afterEach(() => {
  while (wrappers.length) wrappers.pop().unmount()
})

beforeEach(() => {
  listLogs.mockReset()
  listLogFilterOptions.mockReset()
  loadDebugLogging.mockReset()
  setLocale('zh-CN')
  listLogs.mockResolvedValue({ logs: [], total: 0 })
  listLogFilterOptions.mockResolvedValue(OPTIONS)
})

describe('log filter dropdowns', () => {
  it('renders one dropdown per filter and offers the values the server sent', async () => {
    const w = await mountLogs()
    expect(selects(w)).toHaveLength(5)
    // The initial options request carries the window, and no filters.
    expect(listLogFilterOptions.mock.calls.at(-1)[0]).toBe(1)
    expect(lastOptionParams()).toEqual({})
    // Every value ends up rendered as an option somewhere.
    const labels = [...document.querySelectorAll('.el-select-dropdown__item')].map(
      (o) => o.textContent
    )
    for (const v of ['10.0.0.1', 'bob-token', 'my-gpt4o', 'gpt-4o-2024', '200', '429']) {
      expect(labels).toContain(v)
    }
  })

  it('applies a pick immediately and asks for fresh options', async () => {
    const w = await mountLogs()
    await pick(w, 0, '10.0.0.1')

    expect(lastLogParams()).toEqual({ ip: '10.0.0.1' })
    // The facet rule lives server-side; the client just re-asks with the new
    // selection so the other four lists narrow to match.
    expect(lastOptionParams()).toEqual({ ip: '10.0.0.1' })
  })

  it('maps each dropdown to its query parameter and combines them', async () => {
    const w = await mountLogs()
    await pick(w, 0, '10.0.0.1')
    await pick(w, 1, 'carol-token')
    await pick(w, 2, 'my-gpt4o')
    await pick(w, 3, 'gpt-4o-2024')
    await pick(w, 4, '429')

    expect(lastLogParams()).toEqual({
      ip: '10.0.0.1',
      token: 'carol-token',
      model: 'my-gpt4o',
      upstream_model: 'gpt-4o-2024',
      status: 429,
    })
  })

  it('goes back to page 1 when a filter changes', async () => {
    const w = await mountLogs()
    w.vm.page = 3
    await pick(w, 0, '10.0.0.1')

    // Page 3 of the old result set is meaningless in the filtered one.
    expect(listLogs.mock.calls.at(-1)[0]).toBe(1)
  })

  it('refetches the dropdowns when the time window changes', async () => {
    const w = await mountLogs()
    await w.findAll('.toolbar .el-radio-button')[3].trigger('click')
    await flushPromises()

    expect(listLogFilterOptions.mock.calls.at(-1)[0]).toBe(0)
    expect(lastOptionParams()).toEqual({})
  })

  it('clears every dropdown on reset', async () => {
    const w = await mountLogs()
    await pick(w, 0, '10.0.0.1')
    await pick(w, 2, 'my-gpt4o')
    expect(lastLogParams()).toEqual({ ip: '10.0.0.1', model: 'my-gpt4o' })

    // The filter row has exactly one button, and it's disabled until a
    // dropdown holds something.
    const reset = w.find('.filter-row button')
    expect(reset.attributes('disabled')).toBeUndefined()
    await reset.trigger('click')
    await flushPromises()

    expect(lastLogParams()).toEqual({})
    expect(lastOptionParams()).toEqual({})
    expect(w.find('.filter-row button').attributes('disabled')).toBeDefined()
  })

  it('survives an options request that fails', async () => {
    // A dropdown that can't load is still a usable page — the list is what
    // matters, and the error surfaces through the axios interceptor.
    listLogFilterOptions.mockRejectedValue(new Error('boom'))
    const w = await mountLogs()
    expect(listLogs).toHaveBeenCalled()
    expect(selects(w)).toHaveLength(5)
  })
})

describe('status tag', () => {
  // Three rows that exercise the three visible combinations: a clean success,
  // a real upstream failure (non-2xx status), and a 200 whose body carried
  // an error event. The last one is the case B / A add — it has to read as
  // a failure even though the client received 200.
  const rows = [
    { id: 1, token_name: 't', model: 'gpt-4o', channel_name: 'ch', upstream_model: 'gpt-4o',
      status_code: 200, latency_ms: 100, total_tokens: 5, failed_count: 0,
      created_at: 0, protocol: 'openai', stream: 0, convert: 'none', error: '' },
    { id: 2, token_name: 't', model: 'gpt-4o', channel_name: 'ch', upstream_model: 'gpt-4o',
      status_code: 502, latency_ms: 100, total_tokens: 0, failed_count: 1,
      created_at: 0, protocol: 'openai', stream: 0, convert: 'none', error: '...' },
    { id: 3, token_name: 't', model: 'gpt-4o', channel_name: 'ch', upstream_model: 'gpt-4o',
      status_code: 200, latency_ms: 100, total_tokens: 0, failed_count: 1,
      created_at: 0, protocol: 'anthropic', stream: 1, convert: 'none', error: 'upstream error event inside a 200 stream' },
  ]
  beforeEach(() => {
    listLogs.mockResolvedValue({ logs: rows, total: rows.length })
  })

  it('colours a 200 only when nothing actually failed', async () => {
    const w = await mountLogs()
    const tags = w.findAll('.el-table__row .el-tag')
    expect(tags.length).toBeGreaterThanOrEqual(3)
    // Element Plus applies the type as a class on the root element of the tag.
    const types = tags.map((t) => {
      const cls = t.classes().join(' ')
      if (cls.includes('el-tag--success')) return 'success'
      if (cls.includes('el-tag--danger')) return 'danger'
      return cls
    })
    // Each row has *two* tags (status + the per-attempt "fail-badge"); flatten
    // by counting them in document order. We expect: success, danger, success.
    // Row 3 is a 200 with failed_count=1 (failed once then succeeded) — the
    // final status is 2xx so the status badge should be green.
    const statusTypes = types.filter((t) => t === 'success' || t === 'danger')
    expect(statusTypes).toEqual(['success', 'danger', 'success'])
  })
})