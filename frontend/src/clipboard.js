// `navigator.clipboard` only exists in a secure context (HTTPS, or
// localhost). LiteRouter is routinely opened as `http://192.168.x.x:5173`
// during development and from other machines on the LAN, where the property
// is `undefined` and calling it throws — a silent "the copy button does
// nothing". The legacy `execCommand` path still works there, so fall back
// to it rather than requiring TLS.
//
// Returns whether the text actually reached the clipboard, so callers can
// report a failure instead of claiming success.
export async function copyText(text) {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text)
      return true
    } catch {
      // Permission denied or the document isn't focused — the fallback may
      // still succeed, so don't give up here.
    }
  }
  return legacyCopy(text)
}

function legacyCopy(text) {
  const ta = document.createElement('textarea')
  ta.value = text
  // Off-screen rather than `display: none`: a hidden element cannot be
  // selected, and `execCommand('copy')` copies the selection.
  ta.setAttribute('readonly', '')
  ta.style.position = 'fixed'
  ta.style.top = '-1000px'
  ta.style.opacity = '0'
  document.body.appendChild(ta)
  const selection = document.getSelection()
  const previous = selection && selection.rangeCount > 0 ? selection.getRangeAt(0) : null
  try {
    ta.select()
    ta.setSelectionRange(0, text.length)
    return document.execCommand('copy')
  } catch {
    return false
  } finally {
    document.body.removeChild(ta)
    if (previous && selection) {
      selection.removeAllRanges()
      selection.addRange(previous)
    }
  }
}