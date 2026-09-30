#!/usr/bin/env node
// Locale key parity check.
//
// zh-CN is the single definition of the key structure — en-US must match it
// exactly, at every level of nesting. A missing key does not throw: vue-i18n
// silently falls back to the default locale, so the English UI renders a
// stray Chinese string and nothing anywhere reports an error. That is exactly
// the failure mode that keeps recurring, so it gets a check that runs on every
// commit instead of being noticed by a user.
//
// Plain Node, no test framework: these are two ESM modules that export an
// object, and `import` is the only thing needed to read them.

import zhCN from '../src/i18n/locales/zh-CN.js'
import enUS from '../src/i18n/locales/en-US.js'

/** Every leaf key in a locale, as `a.b.c` paths, plus which are empty. */
function collect(obj, prefix = '', out = { keys: [], empty: [] }) {
  for (const [k, v] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${k}` : k
    if (v !== null && typeof v === 'object' && !Array.isArray(v)) {
      collect(v, path, out)
    } else {
      out.keys.push(path)
      if (typeof v !== 'string' || v.trim() === '') out.empty.push(path)
    }
  }
  return out
}

const zh = collect(zhCN)
const en = collect(enUS)

const missing = zh.keys.filter((k) => !en.keys.includes(k))
const extra = en.keys.filter((k) => !zh.keys.includes(k))
const untranslated = en.keys.filter((k) => {
  const zhVal = k.split('.').reduce((o, p) => (o == null ? o : o[p]), zhCN)
  const enVal = k.split('.').reduce((o, p) => (o == null ? o : o[p]), enUS)
  // A few strings are intentionally identical between the two locales
  // (product names, units, formats like "sk-…"). Only flag a value that is
  // byte-identical *and* looks like natural language — i.e. it contains a
  // space and at least one letter.
  return typeof zhVal === 'string' && zhVal === enVal && /[A-Za-z一-鿿]{2,}\s/.test(enVal)
})

const problems = []
if (missing.length) {
  problems.push(`missing from en-US (${missing.length}):\n  ${missing.join('\n  ')}`)
}
if (extra.length) {
  problems.push(`present in en-US but not zh-CN (${extra.length}):\n  ${extra.join('\n  ')}`)
}
if (zh.empty.length) {
  problems.push(`empty values in zh-CN (${zh.empty.length}):\n  ${zh.empty.join('\n  ')}`)
}
if (untranslated.length) {
  problems.push(`suspiciously identical in both locales (${untranslated.length}):\n  ${untranslated.join('\n  ')}`)
}

if (problems.length) {
  console.error('i18n check FAILED\n')
  console.error(problems.join('\n\n'))
  console.error(`\n${zh.keys.length} keys checked.`)
  process.exit(1)
}

console.log(`i18n check passed — ${zh.keys.length} keys, both locales in sync`)
