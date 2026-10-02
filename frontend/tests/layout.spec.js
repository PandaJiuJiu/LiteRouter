import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ElementPlus from 'element-plus'
import { createRouter, createMemoryHistory } from 'vue-router'
import i18n, { setLocale } from '../src/i18n'
import enUS from '../src/i18n/locales/en-US'
import zhCN from '../src/i18n/locales/zh-CN'

const me = vi.fn()
const logout = vi.fn()
const setLanguage = vi.fn()
const push = vi.fn()
vi.mock('../src/api', () => ({ me, logout, setLanguage }))

const { default: Layout } = await import('../src/views/Layout.vue')

// Imported dynamically: session.js pulls in the mocked api, and a static
// import here would run before the `me` mock exists.
const { resetSession } = await import('../src/session')

/** Routes are lazy-loaded in the real app; stub them so the sidebar renders. */
const stubRouter = () =>
  createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }],
  })

async function mountLayout() {
  const router = stubRouter()
  router.push = push
  const w = mount(Layout, {
    global: { plugins: [ElementPlus, i18n, router] },
    stubs: { 'router-view': true },
  })
  await flushPromises()
  return w
}

/** Sidebar labels, in render order. */
const navLabels = (w) => w.findAll('.el-menu-item').map((n) => n.text())

beforeEach(() => {
  me.mockReset()
  logout.mockReset()
  setLanguage.mockReset()
  push.mockReset()
  resetSession() // Layout reads identity from a module singleton, so the cache
  // has to be cleared between cases or the first test's admin leaks into the rest
  localStorage.clear()
  setLocale('zh-CN')
})

describe('sidebar', () => {
  it('shows an admin every entry', async () => {
    me.mockResolvedValue({ username: 'ada', is_admin: true })
    const w = await mountLayout()
    expect(navLabels(w)).toEqual([
      zhCN.nav.channels,
      zhCN.nav.models,
      zhCN.nav.tokens,
      zhCN.nav.mappings,
      zhCN.nav.usage,
      zhCN.nav.logs,
      zhCN.nav.users,
      zhCN.nav.settings,
    ])
  })

  it('hides the admin-only entries from a regular user', async () => {
    // The backend enforces this too; the point of the guard here is that a
    // regular user isn't shown a nav item that 403s the moment they click it.
    me.mockResolvedValue({ username: 'bob', is_admin: false })
    const w = await mountLayout()
    expect(navLabels(w)).toEqual([
      zhCN.nav.tokens,
      zhCN.nav.usage,
      zhCN.nav.logs,
      zhCN.nav.settings,
    ])
  })

  it('renders the username and its initial', async () => {
    me.mockResolvedValue({ username: 'ada', is_admin: false })
    const w = await mountLayout()
    expect(w.find('.user-name').text()).toBe('ada')
    expect(w.find('.avatar').text()).toBe('A')
  })

  it('degrades to a placeholder when /me fails', async () => {
    // The interceptor redirects on 401; the shell must still render.
    me.mockRejectedValue(new Error('401'))
    const w = await mountLayout()
    expect(w.find('.user-name').text()).toBe('')
    expect(w.find('.avatar').text()).toBe('?')
  })

  it('caches the identity instead of re-reading /me on every navigation', async () => {
    // The identity lives in a module singleton (src/session.js) so four views
    // share one request. The cost is that a *later* failure can't flip a
    // previously-correct admin to non-admin — which is exactly why the cache is
    // dropped explicitly on logout/login instead of being re-read blindly.
    me.mockResolvedValue({ username: 'ada', is_admin: true })
    await mountLayout()
    me.mockRejectedValue(new Error('boom'))
    const w = await mountLayout()
    expect(me).toHaveBeenCalledTimes(1)
    expect(navLabels(w)).toContain(zhCN.nav.users)
  })
})

describe('language switching', () => {
  it('follows the active locale', async () => {
    setLocale('en-US')
    me.mockResolvedValue({ username: 'ada', is_admin: false })
    const w = await mountLayout()
    expect(navLabels(w)[0]).toBe(enUS.nav.tokens)
  })

  it('no longer offers a language switch in the user menu', async () => {
    // 切换语言只有 Settings 页一个入口了；下拉菜单里再放一份，两处会打架。
    me.mockResolvedValue({ username: 'ada', is_admin: false })
    const w = await mountLayout()
    await w.vm.onUserCommand('lang:en-US')
    expect(setLanguage).not.toHaveBeenCalled()
  })
})

describe('user menu commands', () => {
  it('ignores commands that are not in the dropdown', async () => {
    me.mockResolvedValue({ username: 'ada', is_admin: false })
    const w = await mountLayout()
    await w.vm.onUserCommand('something-else')
    expect(setLanguage).not.toHaveBeenCalled()
    expect(logout).not.toHaveBeenCalled()
  })
})

describe('logout', () => {
  it('clears the session and routes to login', async () => {
    logout.mockResolvedValue(undefined)
    me.mockResolvedValue({ username: 'ada', is_admin: false })
    const w = await mountLayout()

    await w.vm.onUserCommand('logout')
    await flushPromises()

    expect(logout).toHaveBeenCalled()
    expect(push).toHaveBeenCalledWith('/login')
  })

  it('still navigates when the logout request fails', async () => {
    // api.js swallows the request error and clears localStorage regardless.
    logout.mockRejectedValue(new Error('401'))
    me.mockResolvedValue({ username: 'ada', is_admin: false })
    const w = await mountLayout()

    await w.vm.onUserCommand('logout')
    await flushPromises()

    expect(push).toHaveBeenCalledWith('/login')
  })
})