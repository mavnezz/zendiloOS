import { i18n } from '@/shared/i18n'

/** Date and time to the second, in the interface language. */
export function formatTimestamp(milliseconds: number): string {
  return new Intl.DateTimeFormat(i18n.global.locale.value, {
    dateStyle: 'short',
    timeStyle: 'medium',
  }).format(milliseconds)
}
