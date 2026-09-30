import { reactive, readonly } from 'vue'
import { getBreakerSnapshot, resetBreaker } from './api'

// Circuit-breaker snapshot is read-only — admins reset via the panel button
// or via the underlying settings. Keeps the state here so the snapshot can
// be refreshed from anywhere (Logs page is the only consumer today, but the
// shape is open to others).

const state = reactive({
  snapshot: [],
  loading: false,
})

export const breaker = readonly(state)

export async function loadBreakerSnapshot() {
  state.loading = true
  try {
    state.snapshot = await getBreakerSnapshot()
  } finally {
    state.loading = false
  }
  return state.snapshot
}

export async function resetAllBreakers() {
  state.loading = true
  try {
    await resetBreaker()
    state.snapshot = []
  } finally {
    state.loading = false
  }
}