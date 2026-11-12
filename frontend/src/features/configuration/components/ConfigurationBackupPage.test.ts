import {
  callsTo,
  cleanup,
  click,
  mountApp,
  on,
  page,
  problem,
  settle,
  type,
} from '@/shared/testing/server'

import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ConfigurationPlan } from '@/shared/api/types'

import ConfigurationBackupPage from './ConfigurationBackupPage.vue'

const reload = vi.hoisted(() => ({ back: true }))
vi.mock('@/shared/lib/passphrase', async (original) => ({
  ...(await original<typeof import('@/shared/lib/passphrase')>()),
  waitForServer: () => Promise.resolve(reload.back),
}))

const PASSPHRASE = 'correct horse battery'

function plan(extra: Partial<ConfigurationPlan> = {}): ConfigurationPlan {
  return {
    enabled: false,
    frequency: 'daily',
    weekday: null,
    hour: 3,
    minute: 0,
    target: null,
    keep: 7,
    passphrase_set: false,
    account: null,
    next_at: null,
    last_run: null,
    passphrase_min: 12,
    keep_max: 60,
    ...extra,
  }
}

function submit(selector: string): Promise<void> {
  document.body.querySelector<HTMLElement>(selector)!.closest('form')!.requestSubmit()
  return settle()
}

async function chooseFile(selector: string, content: string): Promise<void> {
  const input = document.body.querySelector<HTMLInputElement>(selector)!
  Object.defineProperty(input, 'files', {
    value: [new File([content], 'backup.tar.zst.age')],
    configurable: true,
  })
  input.dispatchEvent(new Event('change'))
  await settle()
}

beforeEach(() => {
  cleanup()
  reload.back = false
  on('GET', '/api/v1/configuration/backup', () => plan())
  on('GET', '/api/v1/shares', () => [{ id: 3, name: 'backups' }])
  on('GET', '/api/v1/storage/removable', () => [{ id: '1A2B-3C4D', label: 'STICK' }])
})

describe('the configuration backup', () => {
  it('downloads a backup with the passphrase typed twice', async () => {
    const saved: string[] = []
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      saved.push(this.download)
    })
    URL.createObjectURL = vi.fn(() => 'blob:backup')
    URL.revokeObjectURL = vi.fn()
    on(
      'POST',
      '/api/v1/configuration/backup/download',
      () =>
        new Response('encrypted', {
          headers: {
            'Content-Type': 'application/octet-stream',
            'Content-Disposition':
              'attachment; filename="zendiloos-config-nas-20261112T030000Z.tar.zst.age"',
          },
        }),
    )
    mountApp(ConfigurationBackupPage)
    await settle()

    await type('#download-passphrase', PASSPHRASE)
    await type('#download-repeat', 'something else')
    expect(page()).toContain('The passphrases do not match.')
    await type('#download-repeat', PASSPHRASE)
    await submit('#download-passphrase')

    expect(callsTo('POST', '/api/v1/configuration/backup/download')[0]!.body).toEqual({
      passphrase: PASSPHRASE,
    })
    expect(saved).toEqual(['zendiloos-config-nas-20261112T030000Z.tar.zst.age'])
    expect(document.body.querySelector<HTMLInputElement>('#download-passphrase')!.value).toBe('')
  })

  it('saves a plan into a share and backs up now', async () => {
    on('PUT', '/api/v1/configuration/backup', (call) => {
      const body = call.body as { keep: number }
      return plan({
        enabled: true,
        frequency: 'weekly',
        weekday: 6,
        target: { kind: 'share', share_id: 3, folder: 'zendiloOS' },
        keep: body.keep,
        passphrase_set: true,
        account: 'alice',
        next_at: 1_800_000_000,
        last_run: {
          at: 1_799_000_000,
          succeeded: false,
          file: null,
          error: 'No space left on device',
        },
      })
    })
    on('POST', '/api/v1/configuration/backup/run', () => ({ id: 4, kind: 'configuration.backup' }))
    mountApp(ConfigurationBackupPage)
    await settle()

    document.body.querySelector<HTMLButtonElement>('#plan-enabled')!.click()
    await type('#plan-frequency', 'weekly')
    await type('#plan-weekday', '6')
    await type('#plan-time', '04:30')
    await type('#plan-share', '3')
    await type('#plan-keep', '5')
    await type('#plan-passphrase', PASSPHRASE)
    await type('#plan-repeat', PASSPHRASE)
    await submit('#plan-folder')

    expect(callsTo('PUT', '/api/v1/configuration/backup')[0]!.body).toEqual({
      enabled: true,
      frequency: 'weekly',
      weekday: 6,
      hour: 4,
      minute: 30,
      target: { kind: 'share', share_id: 3, folder: 'zendiloOS' },
      keep: 5,
      passphrase: PASSPHRASE,
    })
    expect(page()).toContain('Written as alice.')
    expect(page()).toContain('failed: No space left on device')
    expect(page()).toContain('A passphrase is kept')

    await click('Back up now into the folder')
    expect(callsTo('POST', '/api/v1/configuration/backup/run')).toHaveLength(1)
    expect(page()).toContain('The backup runs')
  })

  it('says what the server refuses of a plan, and a plan without a folder to run', async () => {
    on('PUT', '/api/v1/configuration/backup', () =>
      problem(422, 'invalid_input', 'a plan that is on needs a folder to write to'),
    )
    on('GET', '/api/v1/configuration/backup', () =>
      plan({
        passphrase_set: true,
        target: { kind: 'drive', drive: '1A2B-3C4D', folder: '' },
        last_run: {
          at: 1_799_000_000,
          succeeded: true,
          file: 'zendiloos-config-nas-x.tar.zst.age',
          error: null,
        },
      }),
    )
    on('POST', '/api/v1/configuration/backup/run', () => problem(409, 'not_planned'))
    mountApp(ConfigurationBackupPage)
    await settle()
    expect(page()).toContain('zendiloos-config-nas-x.tar.zst.age')
    expect(document.body.querySelector<HTMLSelectElement>('#plan-drive')!.value).toBe('1A2B-3C4D')
    await submit('#plan-folder')
    expect(page()).toContain('a plan that is on needs a folder to write to')
    await click('Back up now into the folder')
    expect(page()).toContain('Choose a folder and a passphrase for the backups first.')
  })

  it('restores a backup once confirmed and says what came of it', async () => {
    on('POST', '/api/v1/configuration/restore', () => ({
      host_name: 'office',
      created_at: 1_800_000_000,
      version: '0.4.0',
      warnings: [],
      https_port: 8443,
      network: ['05-zendiloos-enp1s0.network'],
    }))
    mountApp(ConfigurationBackupPage)
    await settle()
    await chooseFile('#restore-file', 'encrypted')
    await type('#restore-passphrase', PASSPHRASE)
    await submit('#restore-passphrase')
    expect(callsTo('POST', '/api/v1/configuration/restore')).toHaveLength(0)
    await click('Restore')

    const call = callsTo('POST', '/api/v1/configuration/restore')[0]!
    expect(call.body).toBe('encrypted')
    expect(call.headers.get('x-zendiloos-passphrase')).toBe(btoa(PASSPHRASE))
    expect(page()).toContain('The backup of office')
    expect(page()).toContain('port 8443')
    expect(page()).toContain('05-zendiloos-enp1s0.network')
  })

  it('says why a backup is refused', async () => {
    on('POST', '/api/v1/configuration/restore', () =>
      problem(422, 'backup_too_new', 'schema 60 is newer than 55'),
    )
    mountApp(ConfigurationBackupPage)
    await settle()
    await chooseFile('#restore-file', 'encrypted')
    await type('#restore-passphrase', PASSPHRASE)
    await submit('#restore-passphrase')
    await click('Restore')
    expect(page()).toContain('Update zendiloOS first')
  })
})
