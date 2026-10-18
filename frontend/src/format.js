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
 * Short form for axis ticks: 1_234 -> "1.2K", 5_000_000 -> "5M". Keeps the
 * y-axis labels narrow (a token axis regularly reaches seven digits) while
 * staying numeric enough to read at a glance. At most one decimal place, and
 * the decimal is dropped when it is a trailing zero.
 */
export function fmtCompact(n) {
  const v = Number(n || 0)
  const abs = Math.abs(v)
  const scale = abs >= 1e9 ? 1e9 : abs >= 1e6 ? 1e6 : abs >= 1e3 ? 1e3 : 1
  if (scale === 1) return String(v)
  const unit = scale === 1e9 ? 'B' : scale === 1e6 ? 'M' : 'K'
  const scaled = Math.round((v / scale) * 10) / 10
  return `${scaled}${unit}`
}

/**
 * `YYYY-MM-DD` from a start-of-day unix timestamp in **seconds**.
 *
 * Deliberately not `toLocaleDateString()`: that renders per the browser's
 * locale and would disagree with the `YYYY-MM-DD` the backend already stores
 * in `by_day[].day`, so the same day would sort or read differently depending
 * on who is looking.
 *
 * `tzOffsetSecs` shifts the timestamp into a fixed timezone before reading
 * the date parts (seconds east of UTC, e.g. 28800 for Asia/Shanghai). The
 * usage endpoint buckets by local midnight and returns this offset alongside
 * the day keys, so passing it here keeps the chart label on the same
 * calendar day the backend counted.
 */
export function fmtDate(unixDay, tzOffsetSecs = 0) {
  if (!unixDay) return ''
  const shifted = unixDay * 1000 + (tzOffsetSecs || 0) * 1000
  const d = new Date(shifted)
  const y = d.getUTCFullYear()
  const m = String(d.getUTCMonth() + 1).padStart(2, '0')
  const day = String(d.getUTCDate()).padStart(2, '0')
  return `${y}-${m}-${day}`
}