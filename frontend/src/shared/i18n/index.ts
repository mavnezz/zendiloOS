import { createI18n } from 'vue-i18n'

import { ApiProblem } from '@/shared/api/client'

import de from './de'
import en, { type Messages } from './en'

export type Locale = 'de' | 'en'

const STORAGE_KEY = 'zendiloos.locale'

/** English first: other languages only when chosen in the settings. */
function detectLocale(): Locale {
  try {
    const stored = localStorage.getItem(STORAGE_KEY)
    if (stored === 'de' || stored === 'en') return stored
  } catch {
    // Storage unavailable: use the default.
  }
  return 'en'
}

const locale = detectLocale()
document.documentElement.lang = locale

export const i18n = createI18n<[Messages], Locale, false>({
  legacy: false,
  locale,
  fallbackLocale: 'en',
  messages: { de, en },
})

/** Problem codes with their own message. */
const KNOWN_PROBLEMS = [
  'invalid_credentials',
  'account_disabled',
  'invalid_setup_code',
  'setup_completed',
  'csrf_rejected',
  'network',
] as const

function isKnown(code: string): code is (typeof KNOWN_PROBLEMS)[number] {
  return (KNOWN_PROBLEMS as readonly string[]).includes(code)
}

/** A message for the user explaining why a request failed. */
export function problemMessage(error: unknown): string {
  const { t } = i18n.global
  if (!(error instanceof ApiProblem)) return t('errors.unknown', { code: 'client' })
  if (error.code === 'too_many_attempts') {
    return t('errors.too_many_attempts', {
      minutes: Math.ceil((error.retryAfterSeconds ?? 60) / 60),
    })
  }
  // The server explains rule violations itself (currently in English).
  if (error.code === 'invalid_input' && error.detail) return error.detail
  if (isKnown(error.code)) return t(`errors.${error.code}`)
  return t('errors.unknown', { code: error.code })
}
