import { beforeEach, describe, expect, it, vi } from 'vitest'

const me = vi.fn()
vi.mock('../src/api', () => ({ me }))

const { session, loadSession, resetSession } = await import('../src/session')

beforeEach(() => {
  me.mockReset()
  resetSession()
})

describe('loadSession', () => {
  it('exposes the username and admin flag', async () => {
    me.mockResolvedValue({ username: 'ada', is_admin: true })
    await loadSession()
    expect(session.username).toBe('ada')
    expect(session.isAdmin).toBe(true)
  })

  it('asks /me exactly once, no matter how many callers', async () => {
    // Four views mount per navigation; without the cache that is four requests
    // and four chances for the answers to disagree mid-render.
    me.mockResolvedValue({ username: 'ada', is_admin: true })
    await Promise.all([loadSession(), loadSession(), loadSession()])
    await loadSession()
    expect(me).toHaveBeenCalledTimes(1)
  })

  it('falls back to signed-out on failure, without throwing', async () => {
    // Callers are bare `onMounted(loadSession)` hooks — a rejection here would
    // be an unhandled rejection, not a handled error path.
    me.mockRejectedValue(new Error('401'))
    await expect(loadSession()).resolves.toBeDefined()
    expect(session.username).toBe('')
    expect(session.isAdmin).toBe(false)
  })

  it('does not retry after a failed load', async () => {
    me.mockRejectedValue(new Error('offline'))
    await loadSession()
    await loadSession()
    expect(me).toHaveBeenCalledTimes(1)
  })

  it('treats a missing is_admin as non-admin', async () => {
    me.mockResolvedValue({ username: 'bob' })
    await loadSession()
    expect(session.isAdmin).toBe(false)
  })
})

describe('resetSession', () => {
  it('forces the next load to ask again', async () => {
    // This is what login/logout rely on: the cached identity can be from a
    // different user than the one who just authenticated.
    me.mockResolvedValue({ username: 'ada', is_admin: true })
    await loadSession()
    resetSession()
    me.mockResolvedValue({ username: 'bob', is_admin: false })
    await loadSession()
    expect(me).toHaveBeenCalledTimes(2)
    expect(session.username).toBe('bob')
    expect(session.isAdmin).toBe(false)
  })
})

describe('the exported state', () => {
  it('is readonly, so a view cannot fake an admin flag', async () => {
    me.mockResolvedValue({ username: 'bob', is_admin: false })
    await loadSession()
    // Vue's `readonly` refuses the write (and warns) rather than throwing.
    session.isAdmin = true
    expect(session.isAdmin).toBe(false)
  })
})