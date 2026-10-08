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
  has_debug: true,
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
    buttons.find((b) => /Show captured debug info|查看捕获到的调试信息/.test(b.text())) ||
    buttons.find((b) => b.classes('debug-reload')) ||
    buttons[buttons.length - 1]
  )
}

describe('LogDetail — captured upstream response', () => {
  it('hides the whole section when the log carries no capture', async () => {
    getLog.mockResolvedValue({ ...LOG, has_debug: false })
    const w = await mountDetail()
    await flushPromises()
    // Not just the body — the button goes with it, so a request that captured
    // nothing doesn't leave a dead control behind.
    expect(w.text()).not.toMatch(/Show captured debug info|查看捕获到的调试信息/)
    expect(w.find('pre.debug-body').exists()).toBe(false)
    // And nothing was fetched to find that out.
    expect(getLogDebug).not.toHaveBeenCalled()
  })

  it('shows just the button, no heading or explainer, when a capture exists', async () => {
    const w = await mountDetail()
    await flushPromises()
    expect(w.text()).toMatch(/Show captured debug info|查看捕获到的调试信息/)
    // Nothing narrates the section — the button says it all.
    expect(w.text()).not.toMatch(/Captured upstream response|捕获到的上游响应/)
    expect(w.text()).not.toMatch(/Only the upstream response body is recorded|仅记录上游响应体/)
  })

  it('shows the button when no capture has been fetched yet', async () => {
    getLogDebug.mockResolvedValue({ log_id: 7, available: false })
    const w = await mountDetail()
    await flushPromises()
    expect(w.text()).toMatch(/Show captured debug info|查看捕获到的调试信息/)
    // Body is not rendered yet — the button is.
    expect(w.find('pre.debug-body').exists()).toBe(false)
  })

  it('renders available:true as JSON when the response body parses', async () => {
    getLogDebug.mockResolvedValue({
      log_id: 7,
      available: true,
      request: null,
      response: {
        body: '{"error":{"message":"upstream node is down"}}',
        bytes: 64,
        truncated: false,
        meta: { status: 500 },
        pretty: JSON.stringify({ error: { message: 'upstream node is down' } }, null, 2),
        parseError: false,
      },
      breaker: null,
      meta: {},
    })
    const w = await mountDetail()
    await flushPromises()
    await findLoadButton(w).trigger('click')
    await flushPromises()
    // Should find the response body pre tag
    const pres = w.findAll('pre.debug-body')
    expect(pres.length).toBeGreaterThan(0)
    const pre = pres[pres.length - 1] // last one is response body
    expect(pre.text()).toContain('upstream node is down')
    // Pretty-printed JSON has indentation.
    expect(pre.text()).toMatch(/\n\s+/)
  })

  it('renders available:true verbatim when the response body is not JSON', async () => {
    getLogDebug.mockResolvedValue({
      log_id: 7,
      available: true,
      request: null,
      response: {
        body: '<html>oops</html>',
        bytes: 13,
        truncated: false,
        meta: { status: 200 },
        pretty: '<html>oops</html>',
        parseError: true,
      },
      breaker: null,
      meta: {},
    })
    const w = await mountDetail()
    await flushPromises()
    await findLoadButton(w).trigger('click')
    await flushPromises()
    const pres = w.findAll('pre.debug-body')
    expect(pres.length).toBeGreaterThan(0)
    const pre = pres[pres.length - 1]
    expect(pre.text()).toContain('<html>oops</html>')
    expect(w.text()).toMatch(/Could not format the body as JSON|无法格式化为 JSON/)
  })

  it('reports truncation via the meta and a translated yes/no', async () => {
    getLogDebug.mockResolvedValue({
      log_id: 7,
      available: true,
      request: null,
      response: {
        body: '{"partial":true}',
        bytes: 300000,
        truncated: true,
        meta: { status: 200 },
        pretty: '{"partial":true}',
        parseError: false,
      },
      breaker: null,
      meta: {},
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

  it('shows request section when request is present', async () => {
    getLogDebug.mockResolvedValue({
      log_id: 7,
      available: true,
      request: {
        body: '{"model":"gpt-4o"}',
        bytes: 20,
        truncated: false,
        meta: { url: 'https://api.example.com/v1/chat/completions', method: 'POST' },
        pretty: '{\n  "model": "gpt-4o"\n}',
        parseError: false,
      },
      response: null,
      breaker: null,
      meta: {},
    })
    const w = await mountDetail()
    await flushPromises()
    await findLoadButton(w).trigger('click')
    await flushPromises()
    expect(w.text()).toMatch(/Upstream Request|上游请求/)
    expect(w.text()).toContain('https://api.example.com/v1/chat/completions')
    const pres = w.findAll('pre.debug-body')
    expect(pres.length).toBeGreaterThan(0)
    expect(pres[0].text()).toContain('gpt-4o')
  })

  it('shows breaker section when breaker state is present', async () => {
    getLogDebug.mockResolvedValue({
      log_id: 7,
      available: true,
      request: null,
      response: null,
      breaker: {
        key: 'channel1:gpt-4o',
        is_open: true,
        reason: 'too many 5xx',
        cooldown_remaining_secs: 60,
        current_backoff_secs: 120,
      },
      meta: {},
    })
    const w = await mountDetail()
    await flushPromises()
    await findLoadButton(w).trigger('click')
    await flushPromises()
    expect(w.text()).toMatch(/Circuit Breaker State|熔断器状态/)
    expect(w.text()).toContain('channel1:gpt-4o')
    expect(w.text()).toMatch(/yes|是/)
    expect(w.text()).toContain('too many 5xx')
  })

  it('shows meta section when meta is present', async () => {
    getLogDebug.mockResolvedValue({
      log_id: 7,
      available: true,
      request: null,
      response: null,
      breaker: null,
      meta: {
        channel_name: 'test-channel',
        upstream_model: 'gpt-4o',
        protocol: 'openai',
        convert_mode: 'openai_to_anthropic',
        attempt_number: 1,
        total_attempts: 3,
        client_ip: '1.2.3.4',
        user_agent: 'test-agent',
        token_name: 'test-token',
        is_streaming: false,
        at: 1717000000,
      },
    })
    const w = await mountDetail()
    await flushPromises()
    await findLoadButton(w).trigger('click')
    await flushPromises()
    expect(w.text()).toMatch(/Metadata|元数据/)
    expect(w.text()).toContain('test-channel')
    expect(w.text()).toContain('1 / 3')
  })
})
