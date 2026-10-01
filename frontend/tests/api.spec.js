import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const push = vi.fn()
vi.mock('../src/router', () => ({
  default: { push, currentRoute: { value: { path: '/channels' } } },
}))

const error = vi.fn()
vi.mock('element-plus', () => ({ ElMessage: { error } }))

const apiMod = await import('../src/api')
const { default: api, login, logout } = apiMod

/** Run one request through the real interceptor chain, axios adapter stubbed. */
async function request(config) {
  const adapter = vi.fn(config)
  await api.request({ ...config, adapter }).catch(() => {})
  return adapter
}

/** Same, but the adapter rejects with a shaped axios error. */
async function failing(config, { status = 500, data } = {}) {
  const adapter = vi.fn(async () => {
    const err = new Error('Request failed with status code ' + (status ?? 0))
    err.response = { status, data }
    err.config = config
    throw err
  })
  const err = await api.request({ ...config, adapter }).catch((e) => e)
  return { err, adapter }
}

beforeEach(() => {
  localStorage.clear()
  push.mockReset()
  error.mockReset()
})

afterEach(() => {
  localStorage.clear()
})

describe('request interceptor', () => {
  it('attaches the stored session as a bearer header', async () => {
    localStorage.setItem('session', 'sess-123')
    const adapter = await request({ url: '/tokens', method: 'get' })
    expect(adapter.mock.calls[0][0].headers.Authorization).toBe('Bearer sess-123')
  })

  it('sends no Authorization header when signed out', async () => {
    const adapter = await request({ url: '/tokens', method: 'get' })
    const headers = adapter.mock.calls[0][0].headers
    // AxiosHeaders is case-insensitive; a raw property check would miss it.
    expect(headers.Authorization ?? headers.authorization).toBeUndefined()
  })

  it('preserves headers the caller already set', async () => {
    localStorage.setItem('session', 'sess-123')
    const adapter = await request({ url: '/tokens', method: 'get', headers: { 'X-Trace': 'abc' } })
    const headers = adapter.mock.calls[0][0].headers
    expect(headers['X-Trace']).toBe('abc')
    expect(headers.Authorization ?? headers.authorization).toBe('Bearer sess-123')
  })

  it('overwrites a stale Authorization header rather than appending', async () => {
    localStorage.setItem('session', 'fresh')
    const adapter = await request({
      url: '/tokens',
      method: 'get',
      headers: { Authorization: 'Bearer stale' },
    })
    const headers = adapter.mock.calls[0][0].headers
    expect(headers.Authorization ?? headers.authorization).toBe('Bearer fresh')
  })
})

describe('response interceptor', () => {
  it('passes successful responses through untouched', async () => {
    const adapter = vi.fn(async () => ({ data: { ok: true }, status: 200 }))
    const res = await api.request({ url: '/tokens', method: 'get', adapter })
    expect(res.data).toEqual({ ok: true })
    expect(error).not.toHaveBeenCalled()
    expect(push).not.toHaveBeenCalled()
  })

  it('rejects and surfaces a string error body', async () => {
    const { err } = await failing({ url: '/users', method: 'get' }, {
      status: 400,
      data: { error: '用户名已存在' },
    })
    expect(err).toBeInstanceOf(Error)
    expect(error).toHaveBeenCalledWith('用户名已存在')
  })

  it('unwraps a nested {error: {message}} body', async () => {
    // The OpenAI-compatible upstream relays use this shape.
    await failing({ url: '/channels/fetch-models', method: 'post' }, {
      status: 502,
      data: { error: { message: 'upstream connect error', type: 'api_error' } },
    })
    expect(error).toHaveBeenCalledWith('upstream connect error')
  })

  it('falls back to the axios message when the body carries no error field', async () => {
    await failing({ url: '/logs', method: 'get' }, { status: 502, data: '<html>bad gateway</html>' })
    expect(error).toHaveBeenCalled()
    expect(error.mock.calls[0][0]).toContain('502')
  })

  it('drops the session and routes to /login on 401', async () => {
    localStorage.setItem('session', 'expired')
    await failing({ url: '/tokens', method: 'get' }, { status: 401, data: { error: '未登录' } })
    expect(localStorage.getItem('session')).toBeNull()
    expect(push).toHaveBeenCalledWith('/login')
  })

  it('does not redirect on 403 — a regular user hitting an admin page stays put', async () => {
    // 403 means "logged in, not allowed". Bouncing to /login would look like
    // the session expired and throw away a valid one.
    localStorage.setItem('session', 'valid')
    await failing({ url: '/users', method: 'get' }, { status: 403, data: { error: '无权限' } })
    expect(localStorage.getItem('session')).toBe('valid')
    expect(push).not.toHaveBeenCalled()
  })

  it('does not redirect when the 401 came from the login page itself', async () => {
    // Redirecting to /login while already on /login is the classic
    // infinite-loop; the guard in api.js exists specifically for this.
    const { default: routerStub } = await import('../src/router')
    routerStub.currentRoute.value.path = '/login'
    try {
      localStorage.setItem('session', 'stale')
      const { err } = await failing(
        { url: '/login', method: 'post' },
        { status: 401, data: { error: '用户名或密码错误' } },
      )
      expect(err).toBeInstanceOf(Error)
      expect(push).not.toHaveBeenCalled()
      expect(error).toHaveBeenCalledWith('用户名或密码错误')
    } finally {
      routerStub.currentRoute.value.path = '/channels'
      localStorage.removeItem('session')
    }
  })

  it('shows exactly one message per failed request', async () => {
    await failing({ url: '/usage', method: 'get' }, { status: 500, data: { error: '数据库错误' } })
    expect(error).toHaveBeenCalledTimes(1)
  })
})

describe('session helpers', () => {
  // The exported helpers build their own config (`api.get('/x')`), so there's
  // no per-call slot for an adapter. Swap the instance default instead, then
  // put it back so the interceptor tests above keep their real behaviour.
  let originalAdapter

  beforeEach(() => {
    originalAdapter = api.defaults.adapter
  })

  afterEach(() => {
    api.defaults.adapter = originalAdapter
  })

  it('login stores the session it was handed', async () => {
    api.defaults.adapter = vi.fn(async () => ({
      data: { session: 'sess-9', username: 'ada', is_admin: false },
    }))
    const data = await login('ada', 'password1')
    expect(data.username).toBe('ada')
    expect(localStorage.getItem('session')).toBe('sess-9')
  })

  it('logout clears the session even when the server call fails', async () => {
    // The session table is in-memory server-side; a failure here usually just
    // means it already expired. The local id has to go regardless, or the
    // request interceptor keeps attaching a dead bearer token.
    localStorage.setItem('session', 'sess-9')
    api.defaults.adapter = vi.fn(async () => {
      const err = new Error('nope')
      err.response = { status: 401, data: {} }
      throw err
    })
    await logout()
    expect(localStorage.getItem('session')).toBeNull()
  })

  it('logout clears the session on the happy path too', async () => {
    localStorage.setItem('session', 'sess-9')
    api.defaults.adapter = vi.fn(async () => ({ data: {}, status: 204 }))
    await logout()
    expect(localStorage.getItem('session')).toBeNull()
  })

  it('unwraps the list-shaped responses the views expect', async () => {
    // Each view reads `.tokens` / `.channels` / `.mappings` directly, so a
    // change in how these unwrap shows up as an undefined list in the UI
    // rather than as an error.
    api.defaults.adapter = vi.fn(async (c) => {
      const body = {
        '/tokens': { tokens: [{ id: 1 }] },
        '/channels': { channels: [{ id: 7 }] },
        '/mappings': { mappings: [{ id: 3 }] },
        '/users': { users: [{ id: 2 }] },
      }[c.url]
      return { data: body, status: 200 }
    })
    const { listChannels, listMappings, listUsers, listTokens } = apiMod
    expect(await listTokens()).toEqual([{ id: 1 }])
    expect(await listChannels()).toEqual([{ id: 7 }])
    expect(await listMappings()).toEqual([{ id: 3 }])
    expect(await listUsers()).toEqual([{ id: 2 }])
  })

  it('coerces a missing breaker snapshot to an empty array', async () => {
    // Settings.vue renders `breaker.snapshot.length` directly; a null from an
    // older backend would crash the page instead of showing "no open keys".
    api.defaults.adapter = vi.fn(async () => ({ data: {}, status: 200 }))
    const { getBreakerSnapshot } = apiMod
    expect(await getBreakerSnapshot()).toEqual([])
  })

  it('forwards query params on logs and usage', async () => {
    const adapter = vi.fn(async () => ({ data: {}, status: 200 }))
    api.defaults.adapter = adapter
    const { listLogs, fetchUsage } = apiMod
    await listLogs(3, 50, 14)
    expect(adapter.mock.calls[0][0].params).toEqual({ page: 3, size: 50, range: 14 })
    await fetchUsage(30)
        expect(adapter.mock.calls[1][0].params).toEqual({ range: 30 })
  })
})