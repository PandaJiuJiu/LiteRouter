import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ElementPlus from 'element-plus'
import { createRouter, createMemoryHistory } from 'vue-router'
import i18n from '../src/i18n'

const getLog = vi.fn()
vi.mock('../src/api', () => ({ getLog }))
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

describe('LogDetail', () => {
  it('renders the metadata loaded from the API', async () => {
    const w = await mountDetail()
    await flushPromises()
    expect(w.text()).toContain('gpt-4o')
    expect(w.text()).toContain('relay')
  })
})
