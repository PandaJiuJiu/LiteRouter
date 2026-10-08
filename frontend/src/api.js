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

// Global HTTP/HTTPS proxy. Admin only.
// `host` + `port` describe the proxy server; `enabled` is an independent
// on/off switch — setting a server does not implicitly enable the proxy.
// All three are partial-updatable: only fields present in the object are
// sent to the server.
export const getProxySettings = () => api.get('/settings/proxy').then((r) => r.data)
export const setProxySettings = (patch) =>
  api.put('/settings/proxy', patch).then((r) => r.data)

// ---------- config backup / restore ----------
//
// Admin-only. The export side returns a binary blob with the server-stamped
// filename in `Content-Disposition` — we surface both to the SPA so the
// browser saves it under the same name the server suggested.
// The import side takes the file as a base64 string (the SPA decodes via
// FileReader) plus the passphrase; the server decodes, decrypts, and
// reports conflicts without writing anything.

const contentDispositionName = (headerValue) => {
  if (!headerValue) return null
  const m = /filename\*?=(?:UTF-8'')?"([^"]+)"|filename=([^;]+)/i.exec(headerValue)
  const raw = (m && (m[1] || m[2])) || ''
  return raw.replace(/^"|"$/g, '') || null
}

export const exportConfig = (sections, passphrase) =>
  api
    .post('/config/export', { sections, passphrase }, { responseType: 'blob' })
    .then((r) => ({
      blob: r.data,
      name: contentDispositionName(r.headers?.['content-disposition']) || 'literouter-backup.lrbak',
    }))

export const previewImport = ({ file, passphrase }) =>
  api.post('/config/import/preview', { file, passphrase }).then((r) => r.data)

export const commitImport = ({ file, passphrase, decisions }) =>
  api.post('/config/import/commit', { file, passphrase, decisions }).then((r) => r.data)

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
// Append-only history (tripped / re-tripped / recovered / reset_all /
// reset_key). Same shape as the logs endpoint: { events, total }.
export const listBreakerHistory = (page = 1, size = 20, range = 1, filters = {}) =>
  api.get('/breaker/history', { params: { page, size, range, ...filters } }).then((r) => r.data)
// Dropdown values for the channel / model filters; `event` is a closed set
// rendered straight from i18n on the client.
export const listBreakerHistoryFilterOptions = (range = 1, filters = {}) =>
  api
    .get('/breaker/history/filter-options', { params: { range, ...filters } })
    .then((r) => r.data)

export default api