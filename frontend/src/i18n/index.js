import { createI18n } from 'vue-i18n'
import zhCN from './locales/zh-CN'
import enUS from './locales/en-US'

export const LANGUAGES = [
  { value: 'zh-CN', label: '中文' },
  { value: 'en-US', label: 'English' },
]

export const DEFAULT_LOCALE = 'zh-CN'

export function isSupported(lang) {
  return LANGUAGES.some((l) => l.value === lang)
}

export const i18n = createI18n({
  legacy: false,
  globalInjection: true,
  locale: DEFAULT_LOCALE,
  fallbackLocale: DEFAULT_LOCALE,
  messages: { 'zh-CN': zhCN, 'en-US': enUS },
})

/**
 * Switch the active locale and mirror it into <html lang>. Persisting to
 * localStorage keeps the very first paint in the right language on a hard
 * reload — the DB roundtrip in main.js would otherwise flash the fallback.
 */
export function setLocale(lang) {
  const next = isSupported(lang) ? lang : DEFAULT_LOCALE
  i18n.global.locale.value = next
  document.documentElement.lang = next
  localStorage.setItem('literouter.lang', next)
  return next
}

/** Translation outside a component (api.js interceptors). No Vue context needed. */
export function translate(key, params) {
  return i18n.global.t(key, params ?? {})
}

export default i18n