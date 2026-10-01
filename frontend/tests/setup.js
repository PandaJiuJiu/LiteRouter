// jsdom ships neither of these, and both are touched by the code under test:
// `src/i18n` mirrors the locale into <html lang> on every setLocale(), and
// `src/views/Layout.vue` opens the GitHub repo through window.open.
if (!globalThis.matchMedia) {
  globalThis.matchMedia = () => ({
    matches: false,
    addEventListener() {},
    removeEventListener() {},
  })
}

if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {}
}