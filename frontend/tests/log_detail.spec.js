import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ElementPlus from 'element-plus'
import { createRouter, createMemoryHistory } from 'vue-router'
import i18n from '../src/i18n'

const getLog = vi.fn()
const openLogStream = vi.fn(() => ({ close: vi.fn() }))
vi.mock('../src/api', () => ({ getLog, openLogStream }))
vi.mock('../src/session', () => ({ session: { isAdmin: true } }))

const { default: LogDetail } = await import('../src/views/LogDetail.vue')

const LOG = {
  id: 7,
  token_name: 'relay',
  channel_name: 'first',
  status_code: 200,
  prompt_tokens: 4,
  completion_tokens: 2,
  total_tokens: 6,
  cache_read_tokens: 0,
  cache_creation_tokens: 0,
  reasoning_tokens: 0,
  request_model: 'gpt-4o',
  upstream_model: 'gpt-4o',
  latency_ms: 1234,
  stream: false,
  protocol: 'openai',
  convert: 'none',
  error: '',
  failed_count: 0,
  created_at: 1717000000,
  client_ip: '127.0.0.1',
  user_agent: 'test',
  attempts: [],
}

const wrappers = []

async function mountDetail() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/logs/:id', component: { template: '<div />' } }],
  })
  await router.push('/logs/7')
  await router.isReady()
  const w = mount(LogDetail, {
    attachTo: document.body,
    global: { plugins: [ElementPlus, i18n, router] },
  })
  wrappers.push(w)
  return w
}

// The callback the view hands to openLogStream — capturing it lets a test feed
// a "pending" event in and watch the chain react, without an SSE connection.
let onLog

beforeEach(() => {
  getLog.mockReset()
  openLogStream.mockReset()
  openLogStream.mockImplementation((cb) => {
    onLog = cb
    return { close: vi.fn() }
  })
  // Default: metadata load returns a clean log row.
  getLog.mockResolvedValue({ ...LOG })
  // The assertions below match the en-US strings ("In flight").
  i18n.global.locale.value = 'en-US'
  for (const w of wrappers.splice(0)) {
    w.unmount()
  }
  // The Element Plus teleport from one wrapper lingers in document.body
  // after unmount because the leave transition never runs in jsdom. Strip
  // it manually so the next case starts with a clean DOM.
  document.body.querySelectorAll('[data-v-app]').forEach((el) => {
    while (el.firstChild) el.removeChild(el.firstChild)
  })
})

describe('LogDetail', () => {
  it('renders the metadata loaded from the API', async () => {
    const w = await mountDetail()
    await flushPromises()
    expect(w.text()).toContain('gpt-4o')
    expect(w.text()).toContain('relay')
  })

  it('follows a pending request\u2019s relay chain live via SSE', async () => {
    getLog.mockResolvedValue({
      ...LOG,
      pending: true,
      status_code: 0,
      attempts: [],
    })
    const w = await mountDetail()
    await flushPromises()

    // One hop goes in flight: it must appear in the chain as "in flight",
    // not as a status, and the request must stay pending.
    onLog({
      id: 7,
      request_id: 'r1',
      token_name: 'relay',
      request_model: 'gpt-4o',
      upstream_model: 'gpt-4o',
      channel_name: 'ch',
      status_code: 0,
      total_tokens: 0,
      failed_count: 0,
      client_aborted: false,
      pending: true,
      attempts: [
        {
          seq: 0,
          upstream_model: 'gpt-4o',
          channel_name: 'ch',
          status_code: 0,
          error: '',
          latency_ms: 0,
          ok: false,
          skipped: false,
        },
      ],
    })
    await flushPromises()

    expect(w.text()).toContain('ch')
    expect(w.text()).toContain('In flight')
    expect(getLog).toHaveBeenCalledTimes(1)
  })

  it('refetches the full row when the request settles via SSE', async () => {
    getLog.mockResolvedValue({
      ...LOG,
      pending: true,
      status_code: 0,
      attempts: [],
    })
    const w = await mountDetail()
    await flushPromises()

    // The bound log starts "In flight" (en-US locale used above) and the event
    // subscription is set before the final event; a settle must trigger a fresh
    // getLog instead of only merging the list-level event fields.
    onLog({
      id: 7,
      request_id: 'r1',
      token_name: 'relay',
      request_model: 'gpt-4o',
      upstream_model: 'gpt-4o',
      channel_name: 'ch',
      status_code: 200,
      latency_ms: 42,
      total_tokens: 6,
      failed_count: 0,
      client_aborted: false,
      pending: false,
      attempts: [
        {
          seq: 0,
          upstream_model: 'gpt-4o',
          channel_name: 'ch',
          status_code: 200,
          error: '',
          latency_ms: 42,
          ok: true,
          skipped: false,
        },
      ],
    })
    await flushPromises()
    await flushPromises()

    expect(getLog).toHaveBeenCalledTimes(2)
  })
})
