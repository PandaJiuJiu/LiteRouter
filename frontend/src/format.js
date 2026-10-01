// Number and date formatting shared across views. These were byte-identical
// copies in four components, which meant a change (say, switching to a fixed
// locale so the grouping separators don't shift with the browser's settings)
// had to be made four times and three of them would be missed.

/**
 * Thousands-separated integer. Used both directly in templates and as an
 * `el-table-column` formatter — see `UsageTable`'s token columns, which need
 * the three-argument `:formatter` signature.
 */
export function fmtNum(n) {
  return Number(n || 0).toLocaleString()
}

/** `el-table-column` formatter adapter: `(row, column, cellValue) => string`. */
export function tableNum(_row, _column, value) {
  return fmtNum(value)
}

/**
 * `YYYY-MM-DD` from a start-of-day unix timestamp in **seconds**.
 *
 * Deliberately not `toLocaleDateString()`: that renders per the browser's
 * locale and would disagree with the `YYYY-MM-DD` the backend already stores
 * in `by_day[].day`, so the same day would sort or read differently depending
 * on who is looking.
 */
export function fmtDate(unixDay) {
  if (!unixDay) return ''
  const d = new Date(unixDay * 1000)
  const y = d.getFullYear()
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  return `${y}-${m}-${day}`
}