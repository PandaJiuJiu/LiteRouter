import { computed, ref } from 'vue'
import { i18n, setLocale } from './i18n'
import { setLanguage } from './api'

// The language is a site-wide setting with two entry points: the sidebar
// dropdown and the Settings page. Both must behave identically, so the logic
// lives here rather than being copied into each component.
//
// Deliberately kept out of `src/i18n/index.js`: that module is imported by
// `api.js` (for `translate` in the error interceptor), so putting the API call
// there would close an i18n → api → i18n import cycle.

export const languageSaving = ref(false)

/** The locale currently rendered, reactive so templates track changes. */
export const locale = computed(() => i18n.global.locale.value)

/**
 * Apply a language change locally, then persist it.
 *
 * Local-first is deliberate: a language is cosmetic and not worth a spinner,
 * so the UI updates immediately and the write follows. A failed write is
 * *not* rolled back — the interceptor already told the user, and reverting to
 * a language they didn't pick is worse than drifting from the DB until the
 * next load.
 *
 * Persists the value `setLocale` actually applied, not the raw input: an
 * unsupported code normalizes to the default, and the backend rejects
 * anything outside `LANGUAGES`.
 */
export async function switchLanguage(lang) {
  const applied = setLocale(lang)
  languageSaving.value = true
  try {
    await setLanguage(applied)
  } catch (_) {
    // 拦截器已经弹过提示了；本地状态保持用户刚选的，不回滚
  } finally {
    languageSaving.value = false
  }
  return applied
}