import { beforeEach, describe, expect, it, vi } from 'vitest'

// router.js imports { setupStatus } from './api', and api.js imports router
// back — a deliberate cycle that resolves fine at runtime but makes the
// dependency explicit here. Stub the status endpoint rather than standing up
// an axios adapter.
//
// vi.hoisted so the factory below closes over a binding that already exists
// when the module registry is reset between tests.
const { setupStatus } = vi.hoisted(() => ({ setupStatus: vi.fn() }))

let router

/**
 * A fresh router per test.
 *
 * `src/router.js` exports a singleton, and vue-router treats `push` to the
 * current path as a duplicate navigation and skips the guard entirely — so a
 * shared instance would make any test that revisits the path the previous one
 * ended on pass vacuously. Resetting the module registry gets us a new one
 * each time, starting from START_LOCATION.
 */
beforeEach(async () => {
  vi.resetModules()
  setupStatus.mockReset()
  vi.doMock('../src/api', () => ({ setupStatus: () => setupStatus() }))
  router = (await import('../src/router')).default
})

/**
 * Drive one navigation and report where it landed.
 *
 * The guard returns redirect *strings*, so vue-router resolves them on a
 * second internal pass; `isReady()` settles only after both, which is why we
 * await it instead of inspecting the guard's return value directly.
 */
async function navigate(path) {
  await router.push(path).catch(() => {})
  await router.isReady()
  return router.currentRoute.value.path
}

function status({ needsSetup = false, authenticated = false } = {}) {
  return { needsSetup, authenticated, is_admin: false, language: 'zh-CN' }
}

describe('router guard', () => {
  it('sends a fresh install to the setup wizard', async () => {
    setupStatus.mockResolvedValue(status({ needsSetup: true }))
    expect(await navigate('/channels')).toBe('/setup')
  })

  it('keeps the wizard reachable while setup is pending', async () => {
    setupStatus.mockResolvedValue(status({ needsSetup: true }))
    expect(await navigate('/setup')).toBe('/setup')
  })

  it('sends everyone away from /setup once initialized', async () => {
    setupStatus.mockResolvedValue(status({ needsSetup: false }))
    expect(await navigate('/setup')).toBe('/login')
  })

  it('sends a signed-in user from /setup to their dashboard', async () => {
    setupStatus.mockResolvedValue(status({ authenticated: true }))
    expect(await navigate('/setup')).toBe('/channels')
  })

  it('lets a signed-out user reach the login page', async () => {
    setupStatus.mockResolvedValue(status())
    expect(await navigate('/login')).toBe('/login')
  })

  it('bounces a signed-in user off the login page', async () => {
    setupStatus.mockResolvedValue(status({ authenticated: true }))
    expect(await navigate('/login')).toBe('/channels')
  })

  it('guards every non-auth page', async () => {
    setupStatus.mockResolvedValue(status())
    for (const path of [
      '/channels',
      '/models',
      '/usage',
      '/tokens',
      '/mappings',
      '/logs',
      '/logs/42',
      '/users',
      '/settings',
    ]) {
      expect(await navigate(path)).toBe('/login')
    }
  })

  it('admits a signed-in user to every non-auth page', async () => {
    setupStatus.mockResolvedValue(status({ authenticated: true }))
    for (const path of ['/channels', '/tokens', '/logs/42', '/settings']) {
      expect(await navigate(path)).toBe(path)
    }
  })

  it('queries setup-status once per navigation, not once per rule', async () => {
    // The whole point of hoisting the call above the branches is that a
    // redirect must not cost a second roundtrip.
    setupStatus.mockResolvedValue(status({ authenticated: true }))
    await navigate('/usage')
    expect(setupStatus).toHaveBeenCalledTimes(1)
  })

  it('treats needsSetup as dominant over an existing session', async () => {
    // A session can outlive a DB wipe; the wizard has to win or the user lands
    // on a page that will 401 on every call.
    setupStatus.mockResolvedValue(status({ needsSetup: true, authenticated: true }))
    expect(await navigate('/tokens')).toBe('/setup')
  })

  it('treats needsSetup as dominant even for /login', async () => {
    setupStatus.mockResolvedValue(status({ needsSetup: true, authenticated: true }))
    expect(await navigate('/login')).toBe('/setup')
  })

  it('propagates a status-endpoint outage as a rejected navigation', async () => {
    // There is no offline mode; swallowing this would render a blank shell
    // with no way to tell the user the backend is down.
    setupStatus.mockRejectedValue(new Error('Network Error'))
    await expect(router.push('/channels')).rejects.toThrow('Network Error')
  })
})