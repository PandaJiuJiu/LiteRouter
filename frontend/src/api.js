import axios from 'axios'
import { ElMessage } from 'element-plus'
import router from './router'
import { translate } from './i18n'

const api = axios.create({ baseURL: '/api' })

api.interceptors.request.use((config) => {
  const session = localStorage.getItem('session')
  if (session) config.headers.Authorization = `Bearer ${session}`
  return config
})

api.interceptors.response.use(
  (res) => res,
  (err) => {
    if (err.response?.status === 401 && router.currentRoute.value.path !== '/login') {
      localStorage.removeItem('session')
      router.push('/login')
    }
    // backend errors: {error: "message"} / {error: {message}} / OpenAI style
    const e = err.response?.data?.error
    const msg = typeof e === 'string' ? e : e?.message
    ElMessage.error(msg || err.message || translate('common.requestFailed'))
    return Promise.reject(err)
  },
)

// ---------- auth / setup ----------
export async function setupStatus() {
  const { data } = await api.get('/setup-status')
  return data
}

export async function setup(payload) {
  const { data } = await api.post('/setup', payload)
  if (data.session) localStorage.setItem('session', data.session)
  return data
}

export async function login(username, password) {
  const { data } = await api.post('/login', { username, password })
  localStorage.setItem('session', data.session)
  return data
}

export async function logout() {
  try { await api.post('/logout') } catch (_) {}
  localStorage.removeItem('session')
}

export async function me() {
  const { data } = await api.get('/me')
  return data
}

// ---------- user management (admin) ----------
export const listUsers = () => api.get('/users').then((r) => r.data.users)
export const createUser = (u) => api.post('/users', u).then((r) => r.data)
export const updateUser = (id, u) => api.put(`/users/${id}`, u).then((r) => r.data)
export const deleteUser = (id) => api.delete(`/users/${id}`)

// ---------- channels / models / mappings ----------
export const listChannels = () => api.get('/channels').then((r) => r.data.channels)
export const createChannel = (c) => api.post('/channels', c)
export const updateChannel = (id, c) => api.put(`/channels/${id}`, c)
export const deleteChannel = (id) => api.delete(`/channels/${id}`)
export const fetchModels = (c) =>
  api.post('/channels/fetch-models', c).then((r) => r.data)
export const testModel = (c) =>
  api.post('/channels/test-model', c).then((r) => r.data)
// Set the channel's enabled + disabled model lists without touching any
// other column. The proxy only reads `models` for routing; `disabled_models`
// keeps the off-list so the UI can still render and re-enable those cards
// after a page refresh.
export const updateChannelModels = (id, models, disabled_models) =>
  api.post(`/channels/${id}/models`, { models, disabled_models })

// ---------- tokens ----------
export const listTokens = () => api.get('/tokens').then((r) => r.data.tokens)
export const createToken = (t) => api.post('/tokens', t).then((r) => r.data)
export const updateToken = (id, t) => api.put(`/tokens/${id}`, t).then((r) => r.data)
export const deleteToken = (id) => api.delete(`/tokens/${id}`)

// ---------- logs / usage ----------
export const listLogs = (page = 1, size = 20, range = 1, filters = {}) =>
  api.get('/logs', { params: { page, size, range, ...filters } }).then((r) => r.data)
export const getLog = (id) => api.get(`/logs/${id}`).then((r) => r.data.log)
// Captured upstream response body for one log. `available` says whether the
// relay wrote anything — failures always capture, healthy traffic only
// when the debug switch is on. Body is returned as a string; pretty-print
// on the client (see LogDetail.vue).
export const getLogDebug = (id) => api.get(`/logs/${id}/debug`).then((r) => r.data)
export const listLogFilterOptions = (range = 1, filters = {}) =>
  api.get('/logs/filter-options', { params: { range, ...filters } }).then((r) => r.data)
export const fetchUsage = (range = 7) =>
  api.get('/usage', { params: { range } }).then((r) => r.data)

// ---------- mappings ----------
export const listChannelModels = () => api.get('/models').then((r) => r.data.channels)
export const listMappings = () => api.get('/mappings').then((r) => r.data.mappings)
export const createMapping = (m) => api.post('/mappings', m)
export const updateMapping = (id, m) => api.put(`/mappings/${id}`, m)
export const deleteMapping = (id) => api.delete(`/mappings/${id}`)

// ---------- settings ----------
export const getDebugLogging = () => api.get('/settings/debug-logging').then((r) => r.data.enabled)
export const setDebugLogging = (enabled) =>
  api.put('/settings/debug-logging', { enabled }).then((r) => r.data.enabled)
// Global UI language. Any signed-in user may set it; the login/setup pages
// read the same value from /setup-status since they have no session yet.
export const setLanguage = (language) =>
  api.put('/settings/language', { language }).then((r) => r.data.language)
// How many days of logs to keep. Admin only — the backend enforces 1..=90
// and the Settings page only offers 7/14/30/90; the API never lies about
// the current ceiling so a future server-side bump shows up here.
export const getLogRetention = () =>
  api.get('/settings/log-retention-days').then((r) => r.data)
export const setLogRetention = (days) =>
  api.put('/settings/log-retention-days', { days }).then((r) => r.data.days)

// ---------- circuit breaker ----------
export const getBreakerConfig = () =>
  api.get('/settings/breaker').then((r) => r.data)
export const getBreakerSnapshot = () =>
  api.get('/breaker/snapshot').then((r) => r.data.snapshot || [])
export const resetBreaker = () =>
  api.post('/breaker/reset').then((r) => r.data)
// Probes every tracked key now, ignoring the cooldown. The response carries
// the fresh snapshot too, so the caller can re-render without a second call.
export const probeBreakerNow = () =>
  api.post('/breaker/probe-now').then((r) => r.data)

export default api