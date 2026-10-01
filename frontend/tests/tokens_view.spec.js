import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import ElementPlus from 'element-plus'
import { createRouter, createMemoryHistory } from 'vue-router'
import i18n, { setLocale } from '../src/i18n'
import zhCN from '../src/i18n/locales/zh-CN'

const listTokens = vi.fn()
const createToken = vi.fn()
const updateToken = vi.fn()
const deleteToken = vi.fn()
const listUsers = vi.fn()
const me = vi.fn()
vi.mock('../src/api', () => ({
  listTokens,
  createToken,
  updateToken,
  deleteToken,
  listUsers,
  me,
}))

const { warning, success, error } = vi.hoisted(() => ({
  warning: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
}))
vi.mock('element-plus', async (orig) => {
  const actual = await orig()
  // Keep every other ElMessage method (info/warning/...) real; only the three
  // the view calls on these paths become spies.
  return { ...actual, ElMessage: { ...actual.ElMessage, warning, success, error } }
})

const { default: Tokens } = await import('../src/views/Tokens.vue')

// Imported dynamically: session.js pulls in the mocked api, and a static
// import here would run before the `me` mock exists.
const { resetSession } = await import('../src/session')

const TOKENS = [
  {
    id: 1,
    name: 'ci',
    key: 'sk-aaaa',
    enabled: 1,
    rpm_limit: 60,
    daily_token_limit: 0,
    user_id: 1,
    owner: 'ada',
    accessed_at: 0,
  },
  {
    id: 2,
    name: 'batch',
    key: 'sk-bbbb',
    enabled: 0,
    rpm_limit: 0,
    daily_token_limit: 500_000,
    user_id: null,
    owner: null,
    accessed_at: 1_700_000_000,
  },
]

const router = createRouter({
  history: createMemoryHistory(),
  routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }],
})

async function mountTokens() {
  const w = mount(Tokens, { global: { plugins: [ElementPlus, i18n, router] } })
  await flushPromises()
  return w
}

/** Row text, for "which tokens rendered" assertions. */
const rows = (w) =>
  w.findAll('.el-table__body-wrapper tbody tr').map((r) =>
    r.findAll('td').map((c) => c.text()),
  )

beforeEach(() => {
  for (const m of [listTokens, createToken, updateToken, deleteToken, listUsers, me]) m.mockReset()
  for (const m of [warning, success, error]) m.mockReset()
  resetSession() // the view now reads identity from a module singleton
  setLocale('zh-CN')
  me.mockResolvedValue({ username: 'ada', is_admin: false })
  listTokens.mockResolvedValue(TOKENS)
})

describe('token table', () => {
  it('renders one row per token', async () => {
    await mountTokens()
    expect(rows(await mountTokens())).toHaveLength(2)
  })

  it('labels a zero limit as unlimited rather than rendering "0"', async () => {
    const w = await mountTokens()
    const [ci, batch] = rows(w)
    // ci has rpm 60 / daily 0; batch has rpm 0 / daily 500000.
    expect(ci[3]).toContain('60')
    expect(ci[4]).toBe(zhCN.common.unlimited)
    expect(batch[3]).toBe(zhCN.common.unlimited)
    expect(batch[4]).toContain('500,000')
  })

  it('shows the owner column to an admin only', async () => {
    me.mockResolvedValue({ username: 'ada', is_admin: true })
    listUsers.mockResolvedValue([{ id: 1, username: 'ada', is_admin: true }])
    const w = await mountTokens()
    const headers = w.findAll('.el-table__header th').map((h) => h.text())
    expect(headers.some((h) => h.includes(zhCN.tokens.col.owner))).toBe(true)
    // Admin view also loads the owner picker options.
    expect(listUsers).toHaveBeenCalled()
  })

  it('hides the owner column from a regular user', async () => {
    const w = await mountTokens()
    const headers = w.findAll('.el-table__header th').map((h) => h.text())
    expect(headers.some((h) => h.includes(zhCN.tokens.col.owner))).toBe(false)
    expect(listUsers).not.toHaveBeenCalled()
  })

  it('renders "-" for a token that has never been used', async () => {
    const w = await mountTokens()
    expect(rows(w)[0][5]).toBe('-')
  })
})

describe('create', () => {
  it('refuses a blank name', async () => {
    const w = await mountTokens()
    w.vm.openCreate()
    await flushPromises()
    await w.vm.submit()
    expect(warning).toHaveBeenCalledWith(zhCN.tokens.nameRequired)
    expect(createToken).not.toHaveBeenCalled()
  })

  it('refuses a whitespace-only name', async () => {
    // The check trims; without it a token named "   " would be created and
    // then be indistinguishable from an unnamed one in the sidebar.
    const w = await mountTokens()
    w.vm.openCreate()
    await flushPromises()
    w.vm.form.name = '   '
    await w.vm.submit()
    expect(createToken).not.toHaveBeenCalled()
  })

  it('trims the name before sending it', async () => {
    const w = await mountTokens()
    w.vm.openCreate()
    await flushPromises()
    w.vm.form.name = '  ci-bot  '
    await w.vm.submit()
    expect(createToken).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'ci-bot' }),
    )
  })

  it('clamps negative quota fields to zero', async () => {
    // el-input-number's :min is a UI affordance, not a guarantee — a pasted
    // or restored value can still arrive negative.
    const w = await mountTokens()
    w.vm.openCreate()
    await flushPromises()
    w.vm.form.name = 'neg'
    w.vm.form.rpm_limit = -5
    w.vm.form.daily_token_limit = -1
    await w.vm.submit()
    const payload = createToken.mock.calls[0][0]
    expect(payload.rpm_limit).toBe(0)
    expect(payload.daily_token_limit).toBe(0)
  })

  it('omits user_id for a regular user', async () => {
    // The field isn't rendered for them, so a stale form value must not leak
    // into the payload and silently reassign the token.
    const w = await mountTokens()
    w.vm.openCreate()
    await flushPromises()
    w.vm.form.name = 'mine'
    w.vm.form.user_id = 99
    await w.vm.submit()
    expect(createToken.mock.calls[0][0]).not.toHaveProperty('user_id')
  })

  it('sends user_id for an admin who picked an owner', async () => {
    me.mockResolvedValue({ username: 'ada', is_admin: true })
    listUsers.mockResolvedValue([{ id: 2, username: 'bob', is_admin: false }])
    const w = await mountTokens()
    w.vm.openCreate()
    await flushPromises()
    w.vm.form.name = 'for-bob'
    w.vm.form.user_id = 2
    await w.vm.submit()
    expect(createToken.mock.calls[0][0].user_id).toBe(2)
  })

  it('reloads after a successful create', async () => {
    const w = await mountTokens()
    listTokens.mockClear()
    w.vm.openCreate()
    await flushPromises()
    w.vm.form.name = 'new'
    await w.vm.submit()
    expect(listTokens).toHaveBeenCalled()
    expect(success).toHaveBeenCalled()
  })

  it('leaves the dialog open when the create fails', async () => {
    // The interceptor already surfaced the message; closing the dialog here
    // would throw away everything the user typed.
    createToken.mockRejectedValue(new Error('用户名已存在'))
    const w = await mountTokens()
    w.vm.openCreate()
    await flushPromises()
    w.vm.form.name = 'dup'
    await w.vm.submit().catch(() => {})
    expect(w.vm.dialogVisible).toBe(true)
  })

  it('clears the saving flag on failure', async () => {
    createToken.mockRejectedValue(new Error('boom'))
    const w = await mountTokens()
    w.vm.openCreate()
    await flushPromises()
    w.vm.form.name = 'dup'
    await w.vm.submit().catch(() => {})
    expect(w.vm.saving).toBe(false)
  })
})

describe('edit quota', () => {
  it('sends only the quota fields — not the name', async () => {
    // The backend keys tokens by id and ignores name on update; sending it
    // would imply a rename that never happens.
    const w = await mountTokens()
    w.vm.openEdit(TOKENS[0])
    w.vm.form.rpm_limit = 120
    await w.vm.submit()
    expect(updateToken).toHaveBeenCalledWith(1, {
      enabled: true,
      rpm_limit: 120,
      daily_token_limit: 0,
    })
    expect(updateToken.mock.calls[0][1]).not.toHaveProperty('name')
  })

  it('preserves a disabled token as disabled', async () => {
    const w = await mountTokens()
    w.vm.openEdit(TOKENS[1])
    await w.vm.submit()
    expect(updateToken.mock.calls[0][1].enabled).toBe(false)
  })

  it('never sends user_id when editing', async () => {
    // Reassigning ownership is not part of the quota dialog, so an owner
    // change must not ride along on an edit.
    const w = await mountTokens()
    w.vm.openEdit(TOKENS[0])
    await w.vm.submit()
    expect(updateToken.mock.calls[0][1]).not.toHaveProperty('user_id')
  })
})

describe('inline actions', () => {
  it('keeps the existing quota when toggling enabled', async () => {
    const w = await mountTokens()
    await w.vm.save(TOKENS[0], { enabled: false })
    expect(updateToken).toHaveBeenCalledWith(1, {
      enabled: false,
      rpm_limit: 60,
      daily_token_limit: 0,
    })
  })

  it('reloads after toggling so the switch reflects the server', async () => {
    const w = await mountTokens()
    listTokens.mockClear()
    await w.vm.save(TOKENS[0], { enabled: false })
    expect(listTokens).toHaveBeenCalled()
  })

  it('reloads after a delete', async () => {
    const w = await mountTokens()
    listTokens.mockClear()
    await w.vm.remove(2)
    expect(deleteToken).toHaveBeenCalledWith(2)
    expect(listTokens).toHaveBeenCalled()
  })

  it('copies the key to the clipboard', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.assign(navigator, { clipboard: { writeText } })
    const w = await mountTokens()
    await w.vm.copyKey('sk-aaaa')
    expect(writeText).toHaveBeenCalledWith('sk-aaaa')
    expect(success).toHaveBeenCalledWith(zhCN.tokens.copied)
  })

  it('reports a failure instead of claiming success when the copy throws', async () => {
    // The async write rejects on a rejected permission — the old code fired
    // `copied` without awaiting, so it toasted success either way.
    Object.assign(navigator, {
      clipboard: { writeText: vi.fn().mockRejectedValue(new Error('denied')) },
    })
    document.execCommand = vi.fn().mockReturnValue(false)
    const w = await mountTokens()
    await w.vm.copyKey('sk-aaaa')
    expect(success).not.toHaveBeenCalled()
    expect(error).toHaveBeenCalledWith(zhCN.tokens.copyFailed)
  })

  it('falls back to execCommand when the clipboard API is absent', async () => {
    // `navigator.clipboard` is undefined outside a secure context — the
    // plain-HTTP LAN access this project is usually opened with.
    Object.assign(navigator, { clipboard: undefined })
    const exec = vi.fn().mockReturnValue(true)
    document.execCommand = exec
    const w = await mountTokens()
    await w.vm.copyKey('sk-aaaa')
    expect(exec).toHaveBeenCalledWith('copy')
    expect(success).toHaveBeenCalledWith(zhCN.tokens.copied)
    // The scratch textarea must not be left behind.
    expect(document.querySelectorAll('textarea')).toHaveLength(0)
  })
})

describe('key masking', () => {
  it('masks the key until the eye is clicked', async () => {
    const w = await mountTokens()
    // Column 1 is the key: the owner column is hidden for a non-admin.
    const cell = () => rows(w)[0][1]
    expect(cell()).not.toContain('sk-aaaa')
    expect(cell()).toContain('sk-')

    w.vm.toggleReveal(1)
    await nextTick()
    expect(rows(w)[0][1]).toContain('sk-aaaa')
  })

  it('reveals tokens independently of each other', async () => {
    // One boolean would hide the other key when the admin compares two.
    const w = await mountTokens()
    w.vm.toggleReveal(1)
    await nextTick()
    expect(rows(w)[0][1]).toContain('sk-aaaa')
    expect(rows(w)[1][1]).not.toContain('sk-bbbb')

    w.vm.toggleReveal(1)
    await nextTick()
    expect(rows(w)[0][1]).not.toContain('sk-aaaa')
  })

  it('copies the full key even while it is masked', async () => {
    // Otherwise the eye toggle would gate the button the user needs.
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.assign(navigator, { clipboard: { writeText } })
    const w = await mountTokens()
    await w.vm.copyKey('sk-aaaa')
    expect(writeText).toHaveBeenCalledWith('sk-aaaa')
  })
})

describe('dialog lifecycle', () => {
  it('resets the form after an edit so reopening starts clean', async () => {
    // Otherwise reopening the create dialog would show the previous token's
    // quota — a classic "I typed the wrong number" bug.
    //
    // Exercised through resetForm() rather than by closing the dialog: el-dialog
    // fires `@closed` from its leave transition, which jsdom never runs. The
    // handler binding itself is the one line of template not covered here.
    const w = await mountTokens()
    w.vm.openEdit(TOKENS[0])
    expect(w.vm.form.name).toBe('ci')
    expect(w.vm.form.rpm_limit).toBe(60)

    w.vm.resetForm()
    expect(w.vm.editing).toBeNull()
    expect(w.vm.form.name).toBe('')
    expect(w.vm.form.rpm_limit).toBe(0)
    expect(w.vm.form.enabled).toBe(true)
  })

  it('stays in edit mode when a save fails, keeping the typed values', async () => {
    updateToken.mockRejectedValue(new Error('数据库错误'))
    const w = await mountTokens()
    w.vm.openEdit(TOKENS[0])
    w.vm.form.rpm_limit = 999
    await w.vm.submit().catch(() => {})
    expect(w.vm.editing).not.toBeNull()
    expect(w.vm.form.rpm_limit).toBe(999)
  })
})