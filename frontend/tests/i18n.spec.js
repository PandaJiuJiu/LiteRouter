import { beforeEach, describe, expect, it } from 'vitest'
import i18n, {
  DEFAULT_LOCALE,
  LANGUAGES,
  isSupported,
  setLocale,
  translate,
} from '../src/i18n'
import zhCN from '../src/i18n/locales/zh-CN'
import enUS from '../src/i18n/locales/en-US'

describe('isSupported', () => {
  it('accepts every advertised language', () => {
    for (const l of LANGUAGES) expect(isSupported(l.value)).toBe(true)
  })

  it('rejects anything else', () => {
    expect(isSupported('fr-FR')).toBe(false)
    expect(isSupported('')).toBe(false)
    expect(isSupported(null)).toBe(false)
    expect(isSupported(undefined)).toBe(false)
  })
})

describe('setLocale', () => {
  beforeEach(() => {
    localStorage.clear()
    setLocale(DEFAULT_LOCALE)
  })

  it('switches the active locale and the rendered strings together', () => {
    setLocale('en-US')
    expect(i18n.global.locale.value).toBe('en-US')
    expect(translate('nav.logs')).toBe(enUS.nav.logs)

    setLocale('zh-CN')
    expect(translate('nav.logs')).toBe(zhCN.nav.logs)
  })

  it('mirrors the locale into <html lang> so screen readers and CSS follow', () => {
    setLocale('en-US')
    expect(document.documentElement.lang).toBe('en-US')
    setLocale('zh-CN')
    expect(document.documentElement.lang).toBe('zh-CN')
  })

  it('persists for the next hard reload', () => {
    setLocale('en-US')
    expect(localStorage.getItem('literouter.lang')).toBe('en-US')
  })

  it('falls back to the default for an unsupported language', () => {
    // main.js passes whatever the backend settings row holds. A hand-edited
    // or stale row must not leave the UI in a locale with no messages.
    expect(setLocale('klingon')).toBe(DEFAULT_LOCALE)
    expect(i18n.global.locale.value).toBe(DEFAULT_LOCALE)
    expect(document.documentElement.lang).toBe(DEFAULT_LOCALE)
    expect(localStorage.getItem('literouter.lang')).toBe(DEFAULT_LOCALE)
  })

  it('returns the language it actually applied', () => {
    expect(setLocale('en-US')).toBe('en-US')
    expect(setLocale('en-US')).toBe('en-US')
  })
})

describe('translate', () => {
  it('resolves nested keys outside a component', () => {
    // api.js calls this from the axios error interceptor, where there is no
    // component instance to inject $t into.
    setLocale('zh-CN')
    expect(translate('common.requestFailed')).toBe(zhCN.common.requestFailed)
  })

  it('returns the key itself for an unknown key, without throwing', () => {
    // The interceptor calls translate() on every failed request, including
    // ones caused by a backend that predates a key. It must degrade to the
    // raw key rather than throwing inside the error path.
    expect(() => translate('common.definitelyNotAKey')).not.toThrow()
    expect(translate('common.definitelyNotAKey')).toBe('common.definitelyNotAKey')
  })

  it('falls back to the default locale rather than returning the raw key', () => {
    // Not a substitute for scripts/check-i18n.mjs — this only proves the
    // runtime doesn't render "nav.channels" to a user.
    setLocale('en-US')
    expect(translate('nav.channels')).not.toBe('nav.channels')
    expect(translate('nav.channels')).toBeTruthy()
  })
})