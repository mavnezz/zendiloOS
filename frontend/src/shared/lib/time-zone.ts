// Wall-clock times in a time zone other than the browser's: the NAS's clock
// is shown and set in the NAS's zone, wherever the browser happens to be.

const PATTERN = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/

function parts(at: number, zone: string): Intl.DateTimeFormatPart[] {
  const options: Intl.DateTimeFormatOptions = {
    hourCycle: 'h23',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }
  try {
    return new Intl.DateTimeFormat('en-US', { ...options, timeZone: zone }).formatToParts(at)
  } catch {
    // A zone the browser does not know: UTC rather than nothing.
    return new Intl.DateTimeFormat('en-US', { ...options, timeZone: 'UTC' }).formatToParts(at)
  }
}

/** How far `zone` is ahead of UTC at the moment `at`, in milliseconds. */
export function zoneOffset(at: number, zone: string): number {
  const all = parts(at, zone)
  const get = (type: Intl.DateTimeFormatPartTypes) =>
    Number(all.find((part) => part.type === type)?.value ?? 0)
  const wall = Date.UTC(
    get('year'),
    get('month') - 1,
    get('day'),
    get('hour'),
    get('minute'),
    get('second'),
  )
  return wall - Math.floor(at / 1000) * 1000
}

/** The moment as the clock in `zone` shows it, `2026-11-06T09:30:00`, as a date-time input takes it. */
export function toWallTime(at: number, zone: string): string {
  return new Date(Math.floor(at / 1000) * 1000 + zoneOffset(at, zone)).toISOString().slice(0, 19)
}

/**
 * The moment the clock in `zone` shows `value` (`2026-11-06T09:30`), in
 * milliseconds; `null` if it is not a date and time. In the hour a
 * daylight-saving change repeats, the earlier one.
 */
export function fromWallTime(value: string, zone: string): number | null {
  const match = PATTERN.exec(value)
  if (!match) return null
  const [year = 0, month = 1, day = 1, hour = 0, minute = 0, second = 0] = match
    .slice(1)
    .map((part) => Number(part ?? 0))
  const wall = Date.UTC(year, month - 1, day, hour, minute, second)
  // The offset depends on the moment, which depends on the offset: twice
  // is enough, as zones change their offset at most a few times a year.
  const first = wall - zoneOffset(wall, zone)
  return wall - zoneOffset(first, zone)
}
