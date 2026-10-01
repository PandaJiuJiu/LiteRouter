import { beforeEach, describe, expect, it, vi } from 'vitest'

const getDebugLogging = vi.fn()
const setDebugLogging = vi.fn()
vi.mock('../src/api', () => ({ getDebugLogging, setDebugLogging }))

const { loadDebugLogging, toggleDebugLogging } = await import('../src/debug')

/**
 * `src/debug.js` is a module-level singleton — that's the whole point (the
 * navbar bubble and the Logs-page switch share one flag). Vitest gives each
 * test file a fresh registry, so a `vi.resetModules()` + re-import is the
 * only way to get a clean copy between cases.
 */
async function freshModule() {
  vi.resetModules()
  return import('../src/debug')
}

beforeEach(() => {
  getDebugLogging.mockReset()
  setDebugLogging.mockReset()
})

describe('loadDebugLogging', () => {
  it('caches the first successful read', async () => {
    const m = await freshModule()
    getDebugLogging.mockResolvedValue(true)
    expect(await m.loadDebugLogging()).toBe(true)
    expect(await m.loadDebugLogging()).toBe(true)
    expect(getDebugLogging).toHaveBeenCalledTimes(1)
  })

  it('does not retry after a failed read', async () => {
    // The module comment calls this out: every consumer calls
    // loadDebugLogging on mount, so a backend that's briefly unreachable
    // would otherwise produce one doomed request per component per render.
    const m = await freshModule()
    getDebugLogging.mockRejectedValue(new Error('403'))
    expect(await m.loadDebugLogging()).toBe(false)
    expect(await m.loadDebugLogging()).toBe(false)
    expect(getDebugLogging).toHaveBeenCalledTimes(1)
  })

  it('reports enabled through the read-only export', async () => {
    const m = await freshModule()
    getDebugLogging.mockResolvedValue(true)
    await m.loadDebugLogging()
    expect(m.debugLogging.enabled).toBe(true)
    expect(m.debugLogging.loaded).toBe(true)
  })

  it('leaves a regular user (403 on the admin endpoint) with the switch off', async () => {
    const m = await freshModule()
    getDebugLogging.mockRejectedValue(new Error('无权限'))
    await m.loadDebugLogging()
    expect(m.debugLogging.enabled).toBe(false)
  })

  it('clears the loading flag even when the read fails', async () => {
    const m = await freshModule()
    getDebugLogging.mockRejectedValue(new Error('boom'))
    await m.loadDebugLogging()
    expect(m.debugLogging.loading).toBe(false)
  })

  it('is not writable from outside', async () => {
    const m = await freshModule()
    // `readonly()` is a shallow guard; assigning must be a no-op rather than
    // silently splitting the navbar's view from the Logs page's.
    const before = m.debugLogging.enabled
    m.debugLogging.enabled = true
    expect(m.debugLogging.enabled).toBe(before)
  })
})

describe('toggleDebugLogging', () => {
  it('stores what the server echoed back, not what was clicked', async () => {
    const m = await freshModule()
    setDebugLogging.mockResolvedValue(false)
    // Server-side clamping means the requested value may not stick.
    expect(await m.toggleDebugLogging(true)).toBe(false)
    expect(m.debugLogging.enabled).toBe(false)
  })

  it('sends the requested value through', async () => {
    const m = await freshModule()
    setDebugLogging.mockResolvedValue(true)
    await m.toggleDebugLogging(true)
    expect(setDebugLogging).toHaveBeenCalledWith(true)
  })

  it('reverts the optimistic update when the save fails', async () => {
    const m = await freshModule()
    getDebugLogging.mockResolvedValue(false)
    await m.loadDebugLogging()

    setDebugLogging.mockRejectedValue(new Error('数据库错误'))
    await expect(m.toggleDebugLogging(true)).rejects.toThrow('数据库错误')
    expect(m.debugLogging.enabled).toBe(false)
  })

  it('re-reverts when toggling off fails', async () => {
    const m = await freshModule()
    getDebugLogging.mockResolvedValue(true)
    await m.loadDebugLogging()

    setDebugLogging.mockRejectedValue(new Error('数据库错误'))
    await expect(m.toggleDebugLogging(false)).rejects.toThrow('数据库错误')
    expect(m.debugLogging.enabled).toBe(true)
  })

  it('marks the state loaded so a later mount skips the read', async () => {
    const m = await freshModule()
    setDebugLogging.mockResolvedValue(true)
    await m.toggleDebugLogging(true)
    expect(m.debugLogging.loaded).toBe(true)
    await m.loadDebugLogging()
    expect(getDebugLogging).not.toHaveBeenCalled()
  })

  it('clears the loading flag on the failure path', async () => {
    const m = await freshModule()
    setDebugLogging.mockRejectedValue(new Error('boom'))
    await m.toggleDebugLogging(true).catch(() => {})
    expect(m.debugLogging.loading).toBe(false)
  })
})
