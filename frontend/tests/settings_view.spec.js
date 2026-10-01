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
    // el-select 只渲染当前值，选项在弹层里（jsdom 也不会自动展开），
    // 所以这里读 default 插槽里的 vnode，而不是 DOM。
    const w = await mountSettings()
    // v-for 生成一个 Fragment，default 插槽返回的是嵌套一层的 vnode 数组；
    // el-option 的文案在 props 上。
    const labels = w.findComponent({ name: 'ElSelect' })
      .vm.$slots.default()
      .flatMap((f) => f.children)
      .map((v) => v.props.label)
    expect(labels).toEqual(['中文', 'English'])
  })

  it('shows the current locale in the control', async () => {
    const w = await mountSettings()
    expect(w.find('.el-select__placeholder span').text()).toBe('中文')
  })

  it('lays each setting out as one row: label left, control right', async () => {
    const w = await mountSettings()
    const row = w.find('.row')
    expect(row.find('.row-label').text()).toBe(zhCN.settings.language.title)
    expect(row.find('.el-select').exists()).toBe(true)
  })

  it('switches and persists from the dropdown', async () => {
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