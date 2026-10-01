import { describe, expect, it } from 'vitest'
import { fmtNum, fmtDate, tableNum } from '../src/format'

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

describe('fmtDate', () => {
  // Built from local-midnight `Date` constructors rather than hardcoded unix
  // constants: fmtDate reads local getters, so a literal that looks like
  // "2024-01-05 UTC" is 2024-01-04 (or 01-06) in some CI timezones.
  const at = (y, m, d) => Math.floor(new Date(y, m - 1, d).getTime() / 1000)

  it('formats a start-of-day unix timestamp as YYYY-MM-DD', () => {
    expect(fmtDate(at(2023, 11, 14))).toBe('2023-11-14')
  })

  it('zero-pads single-digit months and days', () => {
    expect(fmtDate(at(2024, 1, 5))).toBe('2024-01-05')
  })

  it('returns empty for a missing day', () => {
    for (const v of [0, null, undefined]) expect(fmtDate(v)).toBe('')
  })

  it('agrees with the YYYY-MM-DD the backend already stores', () => {
    // The API hands back `day` as both a string and a timestamp. If the two
    // ever disagree, a usage row sorts one way in the table and another in a
    // chart. That's why this deliberately avoids toLocaleDateString().
    expect(fmtDate(at(2024, 1, 5)).replace(/-/g, '')).toBe('20240105')
  })
})