import { reactive, readonly } from 'vue'
import { me } from './api'

// Who am I, and am I an admin.
//
// Four views each ran their own `me()` on mount to decide whether to render
// admin-only UI. That's the same request four times per navigation, and the
// answers could disagree for the length of a race. Same shape as
// `src/debug.js`: module-level state, loaded once, shared by everyone.
//
// `Layout.vue` is the shell for all of them, so in principle it could hand
// this down as a prop — but the admin-only *pages* (Users, Channels, …) are
// siblings reached through `<router-view />`, which doesn't forward props
// without extra route config. A shared module is the cheaper wiring.

const state = reactive({
  username: '',
  isAdmin: false,
  loaded: false,
})

export const session = readonly(state)

/**
 * Resolve the current user once and cache it.
 *
 * A failure (401 on an expired session, or 403 for a non-admin hitting an
 * admin-only endpoint) leaves `isAdmin` false — the interceptor has already
 * surfaced the error or redirected. Never throws: every caller is a `onMounted`
 * that would otherwise need its own try/catch to avoid an unhandled rejection.
 */
export async function loadSession() {
  if (state.loaded) return state
  state.loaded = true // set first: a rejected load must not retry every mount
  try {
    const u = await me()
    state.username = u.username ?? ''
    state.isAdmin = !!u.is_admin
  } catch (_) {
    state.username = ''
    state.isAdmin = false
  }
  return state
}

/** Test/debug helper: forget the cached identity. */
export function resetSession() {
  state.loaded = false
  state.username = ''
  state.isAdmin = false
}