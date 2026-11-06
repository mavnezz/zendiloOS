import { describe, expect, it } from 'vitest'

import { fromWallTime, toWallTime, zoneOffset } from './time-zone'

// 2026-11-06 09:00:00 UTC; Berlin is on winter time (UTC+1).
const NOVEMBER = Date.UTC(2026, 10, 6, 9)
// 2026-07-01 12:00:00 UTC; Berlin is on summer time (UTC+2).
const JULY = Date.UTC(2026, 6, 1, 12)

describe('time zones', () => {
  it('knows the offset at each moment', () => {
    expect(zoneOffset(NOVEMBER, 'Europe/Berlin')).toBe(3_600_000)
    expect(zoneOffset(JULY, 'Europe/Berlin')).toBe(7_200_000)
    expect(zoneOffset(JULY, 'Etc/UTC')).toBe(0)
    expect(zoneOffset(JULY, 'America/New_York')).toBe(-4 * 3_600_000)
  })

  it('shows a moment as the zone’s clock does', () => {
    expect(toWallTime(NOVEMBER, 'Europe/Berlin')).toBe('2026-11-06T10:00:00')
    expect(toWallTime(JULY, 'Asia/Kolkata')).toBe('2026-07-01T17:30:00')
  })

  it('reads the zone’s clock back as the moment', () => {
    expect(fromWallTime('2026-11-06T10:00', 'Europe/Berlin')).toBe(NOVEMBER)
    expect(fromWallTime('2026-07-01T14:00:00', 'Europe/Berlin')).toBe(JULY)
    expect(fromWallTime('2026-07-01T12:00', 'Etc/UTC')).toBe(JULY)
    expect(fromWallTime('yesterday', 'Etc/UTC')).toBeNull()
  })

  it('falls back to UTC for a zone the browser does not know', () => {
    expect(zoneOffset(JULY, 'Mars/Olympus')).toBe(0)
  })
})
