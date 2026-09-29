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

export async function login(password) {
  const { data } = await api.post('/login', { password })
  localStorage.setItem('session', data.session)
}

export const listChannels = () => api.get('/channels').then((r) => r.data.channels)
export const createChannel = (c) => api.post('/channels', c)
export const updateChannel = (id, c) => api.put(`/channels/${id}`, c)
export const deleteChannel = (id) => api.delete(`/channels/${id}`)
export const fetchModels = (c) =>
  api.post('/channels/fetch-models', c).then((r) => r.data)
export const testModel = (c) =>
  api.post('/channels/test-model', c).then((r) => r.data)

export const listTokens = () => api.get('/tokens').then((r) => r.data.tokens)
export const createToken = (t) => api.post('/tokens', t).then((r) => r.data)
export const updateToken = (id, t) => api.put(`/tokens/${id}`, t).then((r) => r.data)
export const deleteToken = (id) => api.delete(`/tokens/${id}`)

export const listLogs = (page = 1, size = 50) =>
  api.get('/logs', { params: { page, size } }).then((r) => r.data.logs)
export const fetchUsage = (range = 7) =>
  api.get('/usage', { params: { range } }).then((r) => r.data)

export default api
