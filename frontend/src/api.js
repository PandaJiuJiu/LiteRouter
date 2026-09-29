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
    ElMessage.error(err.response?.data?.error?.message || err.message || '请求失败')
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

export const listTokens = () => api.get('/tokens').then((r) => r.data.tokens)
export const createToken = (t) => api.post('/tokens', t).then((r) => r.data)
export const toggleToken = (id, enabled) => api.put(`/tokens/${id}`, { enabled })
export const deleteToken = (id) => api.delete(`/tokens/${id}`)

export const listLogs = (page = 1, size = 50) =>
  api.get('/logs', { params: { page, size } }).then((r) => r.data.logs)

export default api
