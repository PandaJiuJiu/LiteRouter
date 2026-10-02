import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ElementPlus from 'element-plus'
import { createRouter, createMemoryHistory } from 'vue-router'
import i18n from '../src/i18n'

const getLog = vi.fn()
const getLogDebug = vi.fn()
vi.mock('../src/api', () => ({ getLog, getLogDebug }))
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

beforeEach(() => {
  getLog.mockReset()
  getLogDebug.mockReset()
  // Default: metadata load returns a clean log row.
  getLog.mockResolvedValue({ ...LOG })
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


function findLoadButton(w) {
  const buttons = w.findAll('button')
  return (
    buttons.find((b) => /Show captured response|查看捕获到的响应/.test(b.text())) ||
    buttons.find((b) => b.classes('debug-reload')) ||
    buttons[buttons.length - 1]
  )
}

describe('LogDetail — captured upstream response', () => {
  it('shows the privacy hint and a button when no capture has been fetched', async () => {
    getLogDebug.mockResolvedValue({ log_id: 7, available: false })
    const w = await mountDetail()
    await flushPromises()
    expect(w.text()).toMatch(/Only the upstream response body is recorded|仅记录上游响应体/)
    expect(w.text()).toMatch(/Show captured response|查看捕获到的响应/)
    // Body is not rendered yet — the button is.
    expect(w.find('pre.debug-body').exists()).toBe(false)
  })

  it('renders available:true as JSON when the body parses', async () => {
    getLogDebug.mockResolvedValue({
      log_id: 7,
      available: true,
      bytes: 64,
      truncated: false,
      body: '{"error":{"message":"upstream node is down"}}',
    })
    const w = await mountDetail()
    await flushPromises()
    await w.find('button.debug-reload, button').trigger('click').catch(() => {})
    // Fallback: directly invoke the loader via component (avoids the
    // Element Plus button-text quirk in jsdom).
    await findLoadButton(w).trigger('click')
    await flushPromises()
    const pre = w.find('pre.debug-body')
    expect(pre.exists()).toBe(true)
    expect(pre.text()).toContain('upstream node is down')
    // Pretty-printed JSON has indentation.
    expect(pre.text()).toMatch(/\n\s+/)
  })

  it('renders available:true verbatim when the body is not JSON', async () => {
    getLogDebug.mockResolvedValue({
      log_id: 7,
      available: true,
      bytes: 13,
      truncated: false,
      body: '<html>oops</html>',
    })
    const w = await mountDetail()
    await flushPromises()
    await findLoadButton(w).trigger('click')
    await flushPromises()
    expect(w.find('pre.debug-body').text()).toContain('<html>oops</html>')
    expect(w.text()).toMatch(/Could not format the body as JSON|无法格式化为 JSON/)
  })

  it('reports truncation via the meta and a translated yes/no', async () => {
    getLogDebug.mockResolvedValue({
      log_id: 7,
      available: true,
      bytes: 300000,
      truncated: true,
      body: '{"partial":true}',
    })
    const w = await mountDetail()
    await flushPromises()
    await findLoadButton(w).trigger('click')
    await flushPromises()
    expect(w.text()).toContain('300,000')
    expect(w.text()).toMatch(/Truncated past the capture cap|超过捕获上限已被截断/)
  })

  it('renders the none message and no <pre> when available:false', async () => {
    getLogDebug.mockResolvedValue({ log_id: 7, available: false })
    const w = await mountDetail()
    await flushPromises()
    await findLoadButton(w).trigger('click')
    await flushPromises()
    expect(w.text()).toMatch(/Nothing was captured for this request|本次请求没有捕获到任何内容/)
    expect(w.find('pre.debug-body').exists()).toBe(false)
  })

  it('surfaces an error string from the API without an unhandled rejection', async () => {
    getLogDebug.mockRejectedValue(new Error('boom'))
    const w = await mountDetail()
    await flushPromises()
    await findLoadButton(w).trigger('click')
    await flushPromises()
    // No throw, and the failure is visible.
    expect(w.text()).toContain('boom')
    expect(w.find('pre.debug-body').exists()).toBe(false)
  })
})
