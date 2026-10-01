import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ElementPlus from 'element-plus'
import { createRouter, createMemoryHistory } from 'vue-router'
import i18n, { setLocale } from '../src/i18n'
import zhCN from '../src/i18n/locales/zh-CN'
import enUS from '../src/i18n/locales/en-US'

const setLanguage = vi.fn()
vi.mock('../src/api', () => ({ setLanguage }))

const { default: Settings } = await import('../src/views/Settings.vue')
const { languageSaving, locale, switchLanguage } = await import('../src/language')

const router = createRouter({
  history: createMemoryHistory(),
  routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }],
})

async function mountSettings() {
  const w = mount(Settings, { global: { plugins: [ElementPlus, i18n, router] } })
  await flushPromises()
  return w
}

beforeEach(() => {
  setLanguage.mockReset()
  localStorage.clear()
  setLocale('zh-CN')
  languageSaving.value = false
})

describe('switchLanguage', () => {
  it('applies locally and persists', async () => {
    setLanguage.mockResolvedValue('en-US')
    expect(await switchLanguage('en-US')).toBe('en-US')
    expect(locale.value).toBe('en-US')
    expect(setLanguage).toHaveBeenCalledWith('en-US')
  })

  it('persists the normalized value, not the raw input', async () => {
    // The reason this lives in one shared function: both entry points used to
    // carry their own copy, and only one of them got this right.
    setLanguage.mockResolvedValue('zh-CN')
    expect(await switchLanguage('kl-KL')).toBe('zh-CN')
    expect(setLanguage).toHaveBeenCalledWith('zh-CN')
  })

  it('does not roll back when the persist call fails', async () => {
    setLanguage.mockRejectedValue(new Error('数据库错误'))
    expect(await switchLanguage('en-US')).toBe('en-US')
    expect(locale.value).toBe('en-US')
  })

  it('swallows the rejection so callers need no try/catch', async () => {
    // Both entry points fire this from an event handler with no error path of
    // its own; letting it reject would surface as an unhandled rejection.
    setLanguage.mockRejectedValue(new Error('数据库错误'))
    await expect(switchLanguage('en-US')).resolves.toBe('en-US')
  })

  it('clears the saving flag on both paths', async () => {
    setLanguage.mockResolvedValue('en-US')
    await switchLanguage('en-US')
    expect(languageSaving.value).toBe(false)

    setLanguage.mockRejectedValue(new Error('boom'))
    await switchLanguage('zh-CN')
    expect(languageSaving.value).toBe(false)
  })

  it('tracks the active locale reactively', async () => {
    setLanguage.mockResolvedValue('en-US')
    await switchLanguage('en-US')
    expect(locale.value).toBe('en-US')
    expect(document.documentElement.lang).toBe('en-US')
  })
})

describe('Settings view', () => {
  it('renders the description from the active locale', async () => {
    const w = await mountSettings()
    expect(w.text()).toContain(zhCN.settings.description)
    setLocale('en-US')
    await flushPromises()
    expect(w.text()).toContain(enUS.settings.description)
  })

  it('warns that the setting is site-wide', async () => {
    // The copy makes a promise the backend has to keep: /settings/language is
    // readable and writable by any signed-in user, not just admins.
    const w = await mountSettings()
    expect(w.text()).toContain(zhCN.settings.language.desc)
  })

  it('offers exactly the supported languages', async () => {
    const w = await mountSettings()
    const labels = w.findAll('.el-radio').map((r) => r.text())
    expect(labels).toEqual(['中文', 'English'])
  })

  it('checks the current locale', async () => {
    const w = await mountSettings()
    const checked = w.findAll('.el-radio').filter((r) => r.classes().includes('is-checked'))
    expect(checked).toHaveLength(1)
    expect(checked[0].text()).toBe('中文')
  })

  it('switches and persists from the radio group', async () => {
    setLanguage.mockResolvedValue('en-US')
    const w = await mountSettings()
    await w.vm.onChange('en-US')
    await flushPromises()

    expect(locale.value).toBe('en-US')
    expect(setLanguage).toHaveBeenCalledWith('en-US')
    expect(w.text()).toContain(enUS.settings.description)
  })

  it('no-ops when the picked locale is already active', async () => {
    const w = await mountSettings()
    await w.vm.onChange('zh-CN')
    expect(setLanguage).not.toHaveBeenCalled()
  })

  it('leaves the page translated after a failed write', async () => {
    setLanguage.mockRejectedValue(new Error('数据库错误'))
    const w = await mountSettings()
    await w.vm.onChange('en-US')
    await flushPromises()
    expect(locale.value).toBe('en-US')
  })
})