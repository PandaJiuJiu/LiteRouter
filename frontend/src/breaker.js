import { reactive, readonly } from 'vue'
import { getBreakerSnapshot, resetBreaker, probeBreakerNow } from './api'

// Circuit-breaker snapshot is read-only apart from the admin actions below.
// Keeps the state here so the snapshot can be refreshed from anywhere (the
// Mappings panel is the consumer today, but the shape is open to others).

const state = reactive({
  snapshot: [],
  loading: false,
  // Separate from `loading`: probing actually hits upstreams and can take
  // seconds, so its button needs its own spinner without freezing the
  // refresh button (or the table) behind it.
  probing: false,
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

/// Probe every tracked key now, ignoring the cooldown. Returns
/// `{ probed, recovered }` for the caller to report; the snapshot is
/// replaced in place so the table updates without a follow-up fetch.
export async function probeBreakersNow() {
  state.probing = true
  try {
    const res = await probeBreakerNow()
    if (res.snapshot) state.snapshot = res.snapshot
    return { probed: res.probed || 0, recovered: res.recovered || 0 }
  } finally {
    state.probing = false
  }
}