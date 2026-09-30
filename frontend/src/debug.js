import { reactive, readonly } from 'vue'
import { getDebugLogging, setDebugLogging } from './api'

// Debug-logging is a *global* server setting, but it only has a home on the
// logs page. Both the navbar bubble (Layout) and the switch in the logs
// pagination row (Logs) reflect and flip the same flag, so the state lives
// here rather than in either component — otherwise the two would drift until
// the next remount.
//
// The backend guards these endpoints with require_admin, so a failed load for
// a regular user just leaves the switch off / hidden.

const state = reactive({
  enabled: false,
  loading: false,
  loaded: false,
})

export const debugLogging = readonly(state)

export async function loadDebugLogging() {
  if (state.loaded) return state.enabled
  state.loaded = true // set first: a rejected load must not retry on every render
  try {
    state.enabled = await getDebugLogging()
  } catch (_) {
    state.enabled = false
  }
  return state.enabled
}

export async function toggleDebugLogging(v) {
  state.loading = true
  try {
    const saved = await setDebugLogging(v)
    state.enabled = saved
    state.loaded = true
    return saved
  } catch (e) {
    state.enabled = !v // revert on error
    throw e
  } finally {
    state.loading = false
  }
}
