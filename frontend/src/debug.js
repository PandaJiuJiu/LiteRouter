import { reactive, readonly } from 'vue'
import { getDebugLogging, setDebugLogging } from './api'

// Debug-logging is a *global* server setting, but it only has a home on the
// logs page today. The state lives in its own module rather than in the
// component so the next place that wants to show the same flag (a navbar
// bubble, most likely) can't drift from the switch until a remount.
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
