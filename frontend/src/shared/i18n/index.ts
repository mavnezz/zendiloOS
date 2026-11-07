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

/** Languages offered in the settings, each named in itself. */
export const LANGUAGES: { locale: Locale; name: string }[] = [
  { locale: 'en', name: 'English' },
  { locale: 'de', name: 'Deutsch' },
]

export function setLocale(next: Locale): void {
  i18n.global.locale.value = next
  document.documentElement.lang = next
  try {
    localStorage.setItem(STORAGE_KEY, next)
  } catch {
    // Storage unavailable: applies to this page only.
  }
}

/** Problem codes with their own message. */
const KNOWN_PROBLEMS = [
  'invalid_credentials',
  'wrong_password',
  'forbidden',
  'account_disabled',
  'invalid_setup_code',
  'setup_completed',
  'csrf_rejected',
  'name_taken',
  'last_admin',
  'own_account',
  'password_change_required',
  'not_found',
  'plan_expired',
  'plan_changed',
  'confirmation_mismatch',
  'disk_count',
  'disk_unusable',
  'disk_too_small',
  'pool_not_managed',
  'refused',
  'not_scrubbing',
  'self_test_unsupported',
  'self_test_running',
  'not_testing',
  'delivery_failed',
  'change_pending',
  'not_installed',
  'port_in_use',
  'address_blocked',
  'own_address',
  'address_allowed',
  'project_exists',
  'app_project',
  'not_own_project',
  'project_unconfirmed',
  'app_changed',
  'pool_in_use',
  'jobs_running',
  'disk_not_found',
  'duplicate_disk',
  'no_volume_number',
  'unsupported_layout',
  'device_not_found',
  'docker_installed',
  'not_checked',
  'nothing_to_update',
  'update_busy',
  'password_locked',
  'compose_risks',
  'share_denied',
  'source_exists',
  'official_source',
  'not_a_catalog',
  'catalog_unreachable',
  'app_installed',
  'app_id_too_long',
  'docker_not_ready',
  'permissions_not_confirmed',
  'invalid_app_value',
  'app_busy',
  'up_to_date',
  'host_failed',
  'share_used_by_apps',
  'share_used_by_projects',
  'snapshot_taken',
  'volume_not_found',
  'unknown_subject',
  'duplicate_rule',
  'permission_denied',
  'invalid_operation',
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
  // For these, the server's detail names the disk and what it found.
  const withDetail = [
    'refused',
    'disk_unusable',
    'disk_not_found',
    'duplicate_disk',
    'disk_count',
    'invalid_operation',
    'delivery_failed',
    'not_a_catalog',
    'catalog_unreachable',
    'invalid_app_value',
    'template_unfillable',
    'host_failed',
    'share_used_by_apps',
    'share_used_by_projects',
    'pool_in_use',
    'jobs_running',
  ]
  if (isKnown(error.code)) {
    const message = t(`errors.${error.code}`)
    return withDetail.includes(error.code) && error.detail
      ? `${message} (${error.detail})`
      : message
  }
  return t('errors.unknown', { code: error.code })
}
