import { describe, expect, it } from 'vitest'

import { ApiProblem } from '@/shared/api/client'

import { i18n, problemMessage } from './index'

/** Codes the API sends that once reached people as "unknown". */
const CODES = [
  'wrong_passphrase',
  'not_a_backup',
  'unsupported_backup',
  'backup_too_new',
  'not_planned',
  'disk_not_found',
  'duplicate_disk',
  'no_volume_number',
  'unsupported_layout',
  'device_not_found',
  'docker_installed',
  'not_checked',
  'nothing_to_update',
  'update_busy',
  'update_running',
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
  'would_lock_out',
  'firewall_failed',
  'volume_in_use',
  'device_in_use',
  'volume_busy',
  'containers_held',
  'plan_paused',
  'vms_architecture',
  'kvm_missing',
  'vms_not_installed',
  'vms_installed',
  'vms_not_set_up',
  'vm_creating',
  'vm_not_running',
  'iso_denied',
  'backup_not_installed',
  'backup_installed',
  'ups_not_installed',
  'ups_installed',
  'ups_snmp_missing',
  'ups_password_needed',
  'folder_taken',
  'task_running',
  'task_not_running',
  'restore_running',
  'share_missing',
  'drive_missing',
  'drive_read_only',
  'host_key_changed',
  'agent_failed',
  'newer_snapshots',
  'app_architecture',
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
