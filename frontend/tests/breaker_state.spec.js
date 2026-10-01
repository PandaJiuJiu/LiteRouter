import { beforeEach, describe, expect, it, vi } from 'vitest'

const getBreakerSnapshot = vi.fn()
const resetBreaker = vi.fn()
const probeBreakerNow = vi.fn()
vi.mock('../src/api', () => ({ getBreakerSnapshot, resetBreaker, probeBreakerNow }))

/**
 * Like `src/debug.js`, the breaker snapshot lives in a module-level singleton
 * so it can be refreshed from any view. Each test re-imports for a clean copy.
 */
async function freshModule() {
  vi.resetModules()
  return import('../src/breaker')
}

beforeEach(() => {
  getBreakerSnapshot.mockReset()
  resetBreaker.mockReset()
  probeBreakerNow.mockReset()
})

describe('loadBreakerSnapshot', () => {
  it('stores the snapshot', async () => {
    const m = await freshModule()
    getBreakerSnapshot.mockResolvedValue([{ channel: 'openai', model: 'gpt-4o' }])
    await m.loadBreakerSnapshot()
    expect(m.breaker.snapshot).toHaveLength(1)
    expect(m.breaker.snapshot[0].model).toBe('gpt-4o')
  })

  it('clears the loading flag on success', async () => {
    const m = await freshModule()
    getBreakerSnapshot.mockResolvedValue([])
    await m.loadBreakerSnapshot()
    expect(m.breaker.loading).toBe(false)
  })

  it('clears the loading flag on failure and rethrows', async () => {
    // No swallow here: Logs.vue renders an inline error, and silently leaving
    // the spinner up would hide it.
    const m = await freshModule()
    getBreakerSnapshot.mockRejectedValue(new Error('无权限'))
    await expect(m.loadBreakerSnapshot()).rejects.toThrow('无权限')
    expect(m.breaker.loading).toBe(false)
  })

  it('does not memoize — the panel is a refresh button, not a cached value', async () => {
    const m = await freshModule()
    getBreakerSnapshot.mockResolvedValue([])
    await m.loadBreakerSnapshot()
    getBreakerSnapshot.mockResolvedValue([{ channel: 'x', model: 'y' }])
    await m.loadBreakerSnapshot()
    expect(getBreakerSnapshot).toHaveBeenCalledTimes(2)
    expect(m.breaker.snapshot).toHaveLength(1)
  })

  it('returns the snapshot so callers can use it without reading state', async () => {
    const m = await freshModule()
    const rows = [{ channel: 'a', model: 'b' }]
    getBreakerSnapshot.mockResolvedValue(rows)
    expect(await m.loadBreakerSnapshot()).toEqual(rows)
  })
})

describe('resetAllBreakers', () => {
  it('clears the snapshot after a successful reset', async () => {
    const m = await freshModule()
    getBreakerSnapshot.mockResolvedValue([{ channel: 'a', model: 'b' }])
    await m.loadBreakerSnapshot()

    resetBreaker.mockResolvedValue({ ok: true })
    await m.resetAllBreakers()
    expect(m.breaker.snapshot).toEqual([])
  })

  it('keeps the snapshot when the reset fails', async () => {
    // The whole point of the panel is to see which keys are still open; if the
    // reset call dies we must not paint it as "all clear".
    const m = await freshModule()
    getBreakerSnapshot.mockResolvedValue([{ channel: 'a', model: 'b' }])
    await m.loadBreakerSnapshot()

    resetBreaker.mockRejectedValue(new Error('无权限'))
    await expect(m.resetAllBreakers()).rejects.toThrow('无权限')
    expect(m.breaker.snapshot).toHaveLength(1)
    expect(m.breaker.loading).toBe(false)
  })

  it('is not writable from outside', async () => {
    const m = await freshModule()
    m.breaker.snapshot = [{ channel: 'injected' }]
    expect(m.breaker.snapshot).toEqual([])
  })
})

describe('probeBreakersNow', () => {
  it('adopts the snapshot the response carries, saving a re-fetch', async () => {
    // Recovered keys drop off the server-side list, so the response ships a
    // fresh one — re-fetching would be a wasted round-trip.
    const m = await freshModule()
    getBreakerSnapshot.mockResolvedValue([{ channel: 'a', model: 'b' }])
    await m.loadBreakerSnapshot()
    probeBreakerNow.mockResolvedValue({
      ok: true, probed: 1, recovered: 1, snapshot: [],
    })
    const res = await m.probeBreakersNow()
    expect(res).toEqual({ probed: 1, recovered: 1 })
    expect(m.breaker.snapshot).toEqual([])
    expect(getBreakerSnapshot).toHaveBeenCalledTimes(1)
  })

  it('reports zeroes for a missing or absent count rather than undefined', async () => {
    // The panel renders these straight into a message; `undefined` would show
    // up as literal "undefined" in the toast.
    const m = await freshModule()
    probeBreakerNow.mockResolvedValue({ ok: true, snapshot: [] })
    expect(await m.probeBreakersNow()).toEqual({ probed: 0, recovered: 0 })
  })

  it('clears the probing flag on failure and rethrows', async () => {
    const m = await freshModule()
    probeBreakerNow.mockRejectedValue(new Error('探测失败'))
    await expect(m.probeBreakersNow()).rejects.toThrow('探测失败')
    expect(m.breaker.probing).toBe(false)
  })

  it('keeps probing separate from loading', async () => {
    // Probing hits upstreams and takes seconds; the refresh button and the
    // table must not be stuck behind it.
    const m = await freshModule()
    probeBreakerNow.mockResolvedValue({ ok: true, probed: 0, recovered: 0, snapshot: [] })
    await m.probeBreakersNow()
    expect(m.breaker.probing).toBe(false)
    expect(m.breaker.loading).toBe(false)
  })
})