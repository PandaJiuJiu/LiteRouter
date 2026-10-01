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
  localStorage.clear()
  setLocale('zh-CN')
})

describe('sidebar', () => {
  it('shows an admin every entry', async () => {
    me.mockResolvedValue({ username: 'ada', is_admin: true })
    const w = await mountLayout()
    expect(navLabels(w)).toEqual([
      zhCN.nav.channels,
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

  it('stays signed out rather than rendering a stale admin nav', async () => {
    // The failure path must not fall back to `isAdmin = false` *after* a
    // successful earlier read of admin-only items.
    me.mockResolvedValue({ username: 'ada', is_admin: true })
    await mountLayout()
    me.mockRejectedValue(new Error('boom'))
    const w = await mountLayout()
    expect(navLabels(w)).not.toContain(zhCN.nav.users)
  })
})

describe('language switching', () => {
  it('follows the active locale', async () => {
    setLocale('en-US')
    me.mockResolvedValue({ username: 'ada', is_admin: false })
    const w = await mountLayout()
    expect(navLabels(w)[0]).toBe(enUS.nav.tokens)
  })

  it('applies the switch locally and persists it', async () => {
    me.mockResolvedValue({ username: 'ada', is_admin: false })
    const w = await mountLayout()

    // The dropdown teleports its content, so drive the handler directly.
    await w.vm.onUserCommand('lang:en-US')
    await flushPromises()

    expect(i18n.global.locale.value).toBe('en-US')
    expect(localStorage.getItem('literouter.lang')).toBe('en-US')
    expect(navLabels(w)[0]).toBe(enUS.nav.tokens)
  })

  it('persists the choice to the server so other devices follow', async () => {
    me.mockResolvedValue({ username: 'ada', is_admin: false })
    const w = await mountLayout()
    await w.vm.onUserCommand('lang:en-US')
    await flushPromises()
    expect(setLanguage).toHaveBeenCalledWith('en-US')
  })

  it('keeps the local choice when the persist call fails', async () => {
    // Deliberate, per the component's own comment: a language is cosmetic, and
    // rolling back to a language the user didn't pick is worse than drifting
    // from the DB until next load.
    setLanguage.mockRejectedValue(new Error('数据库错误'))
    me.mockResolvedValue({ username: 'ada', is_admin: false })
    const w = await mountLayout()

    await w.vm.onUserCommand('lang:en-US')
    await flushPromises()
    expect(i18n.global.locale.value).toBe('en-US')
  })

  it('no-ops when the chosen language is already active', async () => {
    setLocale('en-US')
    me.mockResolvedValue({ username: 'ada', is_admin: false })
    const w = await mountLayout()
    await w.vm.onUserCommand('lang:en-US')
    expect(setLanguage).not.toHaveBeenCalled()
  })

  it('normalizes an unsupported language instead of persisting it', async () => {
    // setLocale falls back to the default; persisting the raw string would put
    // a value the backend rejects into settings, 400-ing every later load.
    me.mockResolvedValue({ username: 'ada', is_admin: false })
    const w = await mountLayout()

    await w.vm.onUserCommand('lang:kl-KL')
    await flushPromises()

    expect(i18n.global.locale.value).toBe('zh-CN')
    expect(setLanguage).toHaveBeenCalledWith('zh-CN')
  })

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