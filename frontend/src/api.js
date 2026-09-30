import axios from 'axios'
import { ElMessage } from 'element-plus'
import router from './router'

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
    ElMessage.error(msg || err.message || '请求失败')
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

export async function changePassword(oldPassword, newPassword) {
  await api.post('/password', { old_password: oldPassword, new_password: newPassword })
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

// ---------- tokens ----------
export const listTokens = () => api.get('/tokens').then((r) => r.data.tokens)
export const createToken = (t) => api.post('/tokens', t).then((r) => r.data)
export const updateToken = (id, t) => api.put(`/tokens/${id}`, t).then((r) => r.data)
export const deleteToken = (id) => api.delete(`/tokens/${id}`)

// ---------- logs / usage ----------
export const listLogs = (page = 1, size = 50) =>
  api.get('/logs', { params: { page, size } }).then((r) => r.data)
export const getLog = (id) => api.get(`/logs/${id}`).then((r) => r.data.log)
export const fetchUsage = (range = 7) =>
  api.get('/usage', { params: { range } }).then((r) => r.data)

// ---------- mappings ----------
export const listChannelModels = () => api.get('/models').then((r) => r.data.channels)
export const listMappings = () => api.get('/mappings').then((r) => r.data.mappings)
export const createMapping = (m) => api.post('/mappings', m)
export const updateMapping = (id, m) => api.put(`/mappings/${id}`, m)
export const deleteMapping = (id) => api.delete(`/mappings/${id}`)

export default api