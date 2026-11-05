import { i18n } from '@/shared/i18n'

const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB']

function number(value: number, fractionDigits: number): string {
  return new Intl.NumberFormat(i18n.global.locale.value, {
    maximumFractionDigits: fractionDigits,
  }).format(value)
}

/** Binary units, as disks and memory are measured: 1 KiB = 1024 B. */
export function formatBytes(bytes: number): string {
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${number(value, value < 10 && unit > 0 ? 1 : 0)} ${UNITS[unit]}`
}

export function formatRate(bytesPerSecond: number): string {
  return `${formatBytes(bytesPerSecond)}/s`
}

export function formatPercent(percent: number): string {
  return `${number(percent, percent < 10 ? 1 : 0)} %`
}

/** The two largest units, e.g. `3 d 4 h` or `12 min`. */
export function formatDuration(totalSeconds: number): string {
  const { t } = i18n.global
  const days = Math.floor(totalSeconds / 86_400)
  const hours = Math.floor((totalSeconds % 86_400) / 3600)
  const minutes = Math.floor((totalSeconds % 3600) / 60)
  if (days > 0) return `${t('duration.days', { n: days })} ${t('duration.hours', { n: hours })}`
  if (hours > 0)
    return `${t('duration.hours', { n: hours })} ${t('duration.minutes', { n: minutes })}`
  return t('duration.minutes', { n: minutes })
}

/** Date and time to the second, in the interface language. */
export function formatDateTime(milliseconds: number): string {
  return new Intl.DateTimeFormat(i18n.global.locale.value, {
    dateStyle: 'short',
    timeStyle: 'medium',
  }).format(milliseconds)
}
