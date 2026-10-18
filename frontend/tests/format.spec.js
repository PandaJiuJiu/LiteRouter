import { describe, expect, it } from 'vitest'
import { fmtNum, fmtDate, fmtCompact, tableNum } from '../src/format'

describe('fmtNum', () => {
  it('groups thousands', () => {
    expect(fmtNum(1234567)).toBe((1234567).toLocaleString())
  })

  it('renders 0 for null/undefined/empty rather than "null"', () => {
    // The columns are fed straight from SQLite, where an unset limit is NULL.
    for (const v of [null, undefined, '']) expect(fmtNum(v)).toBe('0')
  })

  it('keeps fractional values', () => {
    expect(fmtNum(1.5)).toBe((1.5).toLocaleString())
  })
})

describe('tableNum', () => {
  it('matches fmtNum via the el-table-column formatter signature', () => {
    // el-table-column calls formatter(row, column, cellValue) — a form that
    // takes the bare value would read `undefined` and print 0 for every cell.
    expect(tableNum({ n: 5 }, { property: 'n' }, 1234)).toBe(fmtNum(1234))
  })
})

describe('fmtCompact', () => {
  it('shortens thousands, millions and billions', () => {
    expect(fmtCompact(1234)).toBe('1.2K')
    expect(fmtCompact(5_000_000)).toBe('5M')
    expect(fmtCompact(2_500_000_000)).toBe('2.5B')
  })

  it('leaves small integers alone so axis zeros read as "0"', () => {
    expect(fmtCompact(0)).toBe('0')
    expect(fmtCompact(999)).toBe('999')
  })

  it('trims a trailing zero decimal', () => {
    expect(fmtCompact(2000)).toBe('2K')
  })
})

describe('fmtDate', () => {
  // Build timestamps from `Date.UTC` so the tests are deterministic in any
  // CI timezone: `fmtDate` reads UTC getters (the timestamp is interpreted
  // as a UTC-day boundary), so a literal that looks like "2024-01-05 UTC"
  // must stay that way regardless of the machine's local time.
  const at = (y, m, d) => Math.floor(Date.UTC(y, m - 1, d) / 1000)

  it('formats a start-of-day unix timestamp as YYYY-MM-DD', () => {
    expect(fmtDate(at(2023, 11, 14))).toBe('2023-11-14')
  })

  it('zero-pads single-digit months and days', () => {
    expect(fmtDate(at(2024, 1, 5))).toBe('2024-01-05')
  })

  it('returns empty for a missing day', () => {
    for (const v of [0, null, undefined]) expect(fmtDate(v)).toBe('')
  })

  it('shifts by a timezone offset before reading the date parts', () => {
    // A UTC-day boundary at 2024-01-05 00:00 UTC is 2024-01-05 08:00 in
    // Asia/Shanghai — the local calendar day is still the 5th.
    expect(fmtDate(at(2024, 1, 5), 8 * 3600)).toBe('2024-01-05')
    // But the same instant is 2024-01-04 16:00 in UTC-8, i.e. the 4th.
    expect(fmtDate(at(2024, 1, 5), -8 * 3600)).toBe('2024-01-04')
    // Offset is applied, then date parts are read in UTC: a timestamp that
    // is the 5th in UTC becomes the 6th when shifted east past midnight.
    expect(fmtDate(at(2024, 1, 5), 24 * 3600)).toBe('2024-01-06')
  })

  it('agrees with the YYYY-MM-DD the backend already stores', () => {
    // The API hands back `day` as both a string and a timestamp. If the two
    // ever disagree, a usage row sorts one way in the table and another in a
    // chart. That's why this deliberately avoids toLocaleDateString().
    expect(fmtDate(at(2024, 1, 5)).replace(/-/g, '')).toBe('20240105')
  })
})