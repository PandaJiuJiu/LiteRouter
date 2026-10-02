import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ElementPlus from 'element-plus'
import { createMemoryHistory, createRouter } from 'vue-router'
import i18n, { setLocale } from '../src/i18n'

const exportConfig = vi.fn()
const previewImport = vi.fn()
const commitImport = vi.fn()

vi.mock('../src/api', () => ({ exportConfig, previewImport, commitImport }))

const { default: ConfigBackup } = await import('../src/views/ConfigBackup.vue')

const router = createRouter({
  history: createMemoryHistory(),
  routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }],
})

const wrappers = []

// `dialogs` lets a spec open one of the two dialogs up-front, so the
// buttons inside the dialog (which the assertions look up via
// `findAll('button')`) are actually mounted in jsdom. Default keeps
// both closed — most tests drive logic through the exposed helpers and
// only need the dialog open when they're asserting on rendered DOM.
async function mountBackup(dialogs = {}) {
  const w = mount(ConfigBackup, {
    props: { exportVisible: !!dialogs.export, importVisible: !!dialogs.import },
    global: { plugins: [ElementPlus, i18n, router] },
  })
  wrappers.push(w)
  await flushPromises()
  return w
}

beforeEach(() => {
  exportConfig.mockReset()
  previewImport.mockReset()
  commitImport.mockReset()
  for (const w of wrappers.splice(0)) w.unmount()
  setLocale('zh-CN')
  // jsdom has no btoa; stub it so arrayBufferToBase64 works in the
  // preview-import flow. Tests that don't go through that path don't care.
  if (!globalThis.btoa) globalThis.btoa = (s) => Buffer.from(s, 'binary').toString('base64')
})

describe('ConfigBackup — export', () => {
  it('blocks the button until a passphrase is long enough and matches', async () => {
    const w = await mountBackup({ export: true })
    const button = w.findAll('button').find((b) => b.text().includes('导出'))
    expect(button.exists()).toBe(true)
    // Length < 8: still off.
    w.vm.setExport({ pass: 'short', confirm: 'short' })
    await flushPromises()
    expect(button.attributes('disabled')).toBeDefined()
    // Length >= 8 and matching: on.
    w.vm.setExport({ pass: 'correct horse battery staple', confirm: 'correct horse battery staple' })
    await flushPromises()
    expect(button.attributes('disabled')).toBeUndefined()
  })

  it('rejects mismatched passphrases with a visible error', async () => {
    const w = await mountBackup({ export: true })
    w.vm.setExport({ pass: 'aaaaaaaa', confirm: 'bbbbbbbb' })
    // Button is disabled when invalid; doExport still surfaces the error.
    await w.vm.doExport()
    expect(w.text()).toMatch(/两次密码不一致|do not match/)
    expect(exportConfig).not.toHaveBeenCalled()
  })

  it('calls exportConfig and triggers a download when everything is right', async () => {
    const w = await mountBackup()
    w.vm.setExport({ pass: 'aaaaaaaa', confirm: 'aaaaaaaa' })
    const fakeBlob = new Blob(['x'])
    exportConfig.mockResolvedValue({ blob: fakeBlob, name: 'backup-2026-10-02.lrbak' })
    // Stub the download path so jsdom doesn't try to navigate.
    const click = vi.fn()
    const origCreate = document.createElement.bind(document)
    const createSpy = vi
      .spyOn(document, 'createElement')
      .mockImplementation((t) => {
        const el = origCreate(t)
        if (t === 'a') el.click = click
        return el
      })
    await w.vm.doExport()
    expect(exportConfig).toHaveBeenCalledWith(
      ['channels', 'tokens', 'mappings'],
      'aaaaaaaa'
    )
    expect(click).toHaveBeenCalled()
    createSpy.mockRestore()
  })
})

describe('ConfigBackup — import preview + commit', () => {
  const PLAN = {
    sections: ['channels', 'tokens'],
    created_at: 1717000000,
    plan: {
      channels: [
        { kind: 'channel', name: 'first', conflict: false, incoming: { base_url: 'http://a' } },
        { kind: 'channel', name: 'second', conflict: true, incoming: { base_url: 'http://b' } },
      ],
      tokens: [
        { kind: 'token', name: 'tok', conflict: false, incoming: { key_prefix: 'sk-abcdef' } },
      ],
      mappings: [],
    },
  }

  it('shows create/update/skip/keep_both counts and a per-row picker', async () => {
    previewImport.mockResolvedValue(PLAN)
    const w = await mountBackup({ import: true })
    // Set the import up before triggering.
    w.vm.setImport({ bytes: 'AAAA', name: 'backup.lrbak', pass: 'aaaaaaaa' })
    await w.vm.doPreview()
    expect(previewImport).toHaveBeenCalled()
    // Counts: 2 creates + 1 conflict (counts as overwrite by default).
    expect(w.text()).toMatch(/新建 2/)
    // Conflict row carries the name and all three action radios.
    const conflictRow = w.find('.conflict-row')
    expect(conflictRow.text()).toContain('second')
    expect(conflictRow.text()).toMatch(/覆盖|Overwrite/)
    expect(conflictRow.text()).toMatch(/跳过|Skip/)
    expect(conflictRow.text()).toMatch(/并存|Keep both/)
  })

  it('refuses commit when a conflict has no decision', async () => {
    previewImport.mockResolvedValue(PLAN)
    commitImport.mockResolvedValue({ created: 0, updated: 0, skipped: 0, kept_both: 0 })
    const w = await mountBackup({ import: true })
    w.vm.setImport({ bytes: 'AAAA', name: 'backup.lrbak', pass: 'aaaaaaaa' })
    await w.vm.doPreview()
    const apply = w.findAll('button').find((b) => b.text().includes('应用'))
    expect(apply.attributes('disabled')).toBeUndefined()
    await w.vm.doCommit()
    expect(commitImport).toHaveBeenCalled()
    const args = commitImport.mock.calls[0][0]
    expect(args.decisions.channels).toEqual([{ name: 'second', action: 'overwrite' }])
  })

  it('sends a keep_both action when the user picks it', async () => {
    previewImport.mockResolvedValue(PLAN)
    commitImport.mockResolvedValue({ created: 0, updated: 0, skipped: 0, kept_both: 1 })
    const w = await mountBackup({ import: true })
    w.vm.setImport({ bytes: 'AAAA', name: 'backup.lrbak', pass: 'aaaaaaaa' })
    await w.vm.doPreview()
    // jsdom can't drive el-radio-button's change handler; poke the
    // decision directly.
    w.vm.setDecision('channels:second', 'keep_both')
    await w.vm.doCommit()
    const args = commitImport.mock.calls[0][0]
    expect(args.decisions.channels[0]).toEqual({ name: 'second', action: 'keep_both' })
  })

  it('surfaces a server error instead of throwing', async () => {
    previewImport.mockRejectedValue({
      response: { data: { error: '密码不对' } },
    })
    const w = await mountBackup({ import: true })
    w.vm.setImport({ bytes: 'AAAA', name: 'backup.lrbak', pass: 'aaaaaaaa' })
    await w.vm.doPreview()
    expect(w.text()).toContain('密码不对')
  })
})