import { describe, expect, it } from 'vitest'

import { ApiProblem } from '@/shared/api/client'

import { i18n, problemMessage } from './index'

/** Codes the API sends that once reached people as "unknown". */
const CODES = [
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
  'in_use',
  'acme_installed',
  'acme_not_installed',
  'web_server_failed',
  'invalid_code',
  'sign_in_expired',
  'two_factor_on',
  'two_factor_off',
  'no_two_factor_setup',
]

describe('problem messages', () => {
  it.each(['en', 'de'] as const)('explain every code the API sends, in %s', (locale) => {
    i18n.global.locale.value = locale
    for (const code of CODES) {
      // An unknown one reads "Something went wrong (<code>)."
      expect(problemMessage(new ApiProblem(409, code)), code).not.toContain(code)
    }
  })

  it('names the disk a plan could not use', () => {
    i18n.global.locale.value = 'en'
    expect(problemMessage(new ApiProblem(422, 'duplicate_disk', 'sdb'))).toContain('(sdb)')
  })
})
