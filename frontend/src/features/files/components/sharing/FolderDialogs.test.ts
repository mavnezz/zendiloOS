import {
  answer,
  callsTo,
  choose,
  cleanup,
  click,
  mountApp,
  on,
  page,
  press,
  problem,
  settle,
  tick,
  type,
} from '@/shared/testing/server'

import { beforeEach, describe, expect, it } from 'vitest'

import { useNotifications } from '@/shared/lib/notifications'
import { account, group, job, share } from '@/shared/testing/fixtures'

import FolderPropertiesDialog from './FolderPropertiesDialog.vue'
import SnapshotsDialog from './SnapshotsDialog.vue'

const photos = share(10, 'photos', {
  description: 'Holidays',
  access: [
    { kind: 'account', id: 5, access: 'read_write' },
    { kind: 'group', id: 1, access: 'read' },
  ],
  nfs: [{ network: '192.168.1.0/24', write: false }],
  settings: { compression: 'zstd', quota_bytes: 10 * 1024 ** 3, record_size_bytes: 1048576 },
})
const empty = share(11, 'empty', { recycle_bin: false, used_bytes: null })

const snapshots = {
  schedules: [{ frequency: 'daily', keep: 7 }],
  snapshots: [
    {
      label: 'zdl-2026-11-07_10.00.00',
      taken_at: 1_793_000_000,
      used_bytes: 4096,
      referenced_bytes: 1024 ** 3,
      schedules: [],
      note: 'Before the move',
    },
    {
      label: 'zdl-2026-11-06_10.00.00',
      taken_at: 1_792_900_000,
      used_bytes: 1024,
      referenced_bytes: 1024 ** 3,
      schedules: ['daily'],
      note: null,
    },
  ],
}

function submit(selector: string): Promise<void> {
  document.body.querySelector<HTMLElement>(selector)!.closest('form')!.requestSubmit()
  return settle()
}

beforeEach(() => {
  cleanup()
  answer('GET', '/api/v1/accounts', [
    account(5, 'anna', { full_name: 'Anna A' }),
    account(6, 'ben'),
  ])
  answer('GET', '/api/v1/groups', [group(1, 'family')])
  answer('GET', '/api/v1/shares/10/snapshots', snapshots)
})

async function properties(of = photos, tab?: 'general' | 'permissions' | 'sharing') {
  await mountApp(FolderPropertiesDialog, { props: { share: of, tab, open: false } }).setProps({
    open: true,
  })
  await settle()
}

async function snapshotsOf(of = photos) {
  await mountApp(SnapshotsDialog, { props: { share: of, open: false } }).setProps({ open: true })
  await settle()
}

describe('the properties of a folder on a volume', () => {
  it('change storage, permissions and sharing, and say when files follow', async () => {
    on('PATCH', '/api/v1/shares/10', () => ({ share: photos, job: job(4, 'sharing.apply_access') }))
    await properties()
    expect(page()).toContain('Properties of photos')
    expect(document.body.querySelector<HTMLInputElement>('#share-quota')!.value).toBe('10')
    await type('#share-quota', '')
    await click('Permissions')
    expect(document.body.querySelector<HTMLSelectElement>('select[aria-label="anna"]')!.value).toBe(
      'read_write',
    )
    await choose('select[aria-label="family"]', '')
    await choose('select[aria-label="ben"]', 'deny')
    await click('Sharing')
    await press('Remove')
    await click('Add computer or network')
    await type('input[aria-label="IP address or network, e.g. 192.168.1.0/24"]', '10.0.0.0/8')
    await tick('Write')
    await tick('Share on the network', false)
    await click('Save')

    expect(callsTo('PATCH', '/api/v1/shares/10')[0]!.body).toEqual({
      description: 'Holidays',
      shared: false,
      recycle_bin: true,
      access: [
        { kind: 'account', id: 5, access: 'read_write' },
        { kind: 'account', id: 6, access: 'deny' },
      ],
      nfs: [{ network: '10.0.0.0/8', write: true }],
      settings: { quota_bytes: null, compression: 'zstd', record_size_bytes: 1048576 },
    })
    expect(useNotifications().items.find((n) => n.title === 'Changes to photos saved')?.body).toBe(
      'Existing files get the new permissions in the background.',
    )
  })

  it('open on the tab asked for, and show a refusal', async () => {
    on('PATCH', '/api/v1/shares/11', () => problem(409, 'refused', 'Macs back up to empty'))
    await properties(empty, 'sharing')
    expect(page()).toContain('Offers empty over SMB')
    await click('Save')
    expect(page()).toContain('Macs back up to empty')
    await click('Cancel')
    expect(document.body.querySelector('[role="dialog"]')).toBeNull()
  })
})

describe('the snapshots dialog', () => {
  it('shows snapshots and schedules, saves schedules and takes one by hand', async () => {
    on('PUT', '/api/v1/shares/10/snapshots/schedules', ({ body }) => body)
    on('POST', '/api/v1/shares/10/snapshots', () => snapshots.snapshots[0])
    await snapshotsOf()
    expect(page()).toContain('Snapshots of photos')
    expect(page()).toContain('Before the move')
    expect(page()).toContain('By hand')
    expect(document.body.querySelector<HTMLInputElement>('#keep-daily')!.disabled).toBe(false)
    expect(document.body.querySelector<HTMLInputElement>('#keep-hourly')!.disabled).toBe(true)
    await tick('Every hour')
    await type('#keep-hourly', '12')
    await tick('Every day', false)
    await click('Save schedules')
    expect(callsTo('PUT', '/api/v1/shares/10/snapshots/schedules')[0]!.body).toEqual([
      { frequency: 'hourly', keep: 12 },
    ])
    expect(page()).toContain('Saved')

    await type('#snapshot-note', 'Now')
    await submit('#snapshot-note')
    expect(callsTo('POST', '/api/v1/shares/10/snapshots')[0]!.body).toEqual({ note: 'Now' })
    expect(document.body.querySelector<HTMLInputElement>('#snapshot-note')!.value).toBe('')
  })

  it('deletes a snapshot after a second click, or not', async () => {
    on('POST', /\/api\/v1\/shares\/10\/snapshots\/.*\/delete/, () => undefined)
    await snapshotsOf()
    const deleteButtons = () => [
      ...document.body.querySelectorAll<HTMLButtonElement>(
        '[role="dialog"] button[aria-label="Delete"]',
      ),
    ]
    deleteButtons()[0]!.click()
    await settle()
    await click('Cancel')
    deleteButtons()[0]!.click()
    await settle()
    const confirm = [
      ...document.body.querySelectorAll<HTMLButtonElement>('[role="dialog"] button'),
    ].find((b) => b.textContent?.trim() === 'Delete')!
    confirm.click()
    await settle()
    expect(callsTo('POST', /\/delete$/)[0]!.path).toBe(
      '/api/v1/shares/10/snapshots/zdl-2026-11-07_10.00.00/delete',
    )
  })

  it('rolls back once the folder is named, warning of later snapshots', async () => {
    on('POST', /\/rollback$/, () => ({ destroyed: ['zdl-2026-11-07_10.00.00'] }))
    await snapshotsOf()
    const rollbacks = document.body.querySelectorAll<HTMLButtonElement>(
      'button[aria-label="Roll back to this snapshot"]',
    )
    rollbacks[1]!.click()
    await settle()
    expect(page()).toContain('The later snapshot is deleted too.')
    await type('#rollback-zdl-2026-11-06_10\\.00\\.00', 'photos')
    await submit('#rollback-zdl-2026-11-06_10\\.00\\.00')
    expect(callsTo('POST', /\/rollback$/)[0]!.body).toEqual({ confirm: 'photos' })
    expect(page()).toContain('The share is back as it was on')

    rollbacks[0]!.click()
    await settle()
    const cancel = [...document.body.querySelectorAll<HTMLButtonElement>('form button')].find(
      (b) => b.textContent?.trim() === 'Cancel',
    )!
    cancel.click()
    await settle()
    expect(document.body.querySelector('[id^="rollback-"]')).toBeNull()
  })

  it('shows what failed', async () => {
    on('POST', '/api/v1/shares/10/snapshots', () => problem(409, 'snapshot_taken'))
    await snapshotsOf()
    await submit('#snapshot-note')
    expect(page()).toContain('A snapshot was taken this second already.')

    cleanup()
    on('GET', '/api/v1/shares/11/snapshots', () => problem(500, 'internal'))
    await snapshotsOf(empty)
    expect(document.body.querySelector('[role="dialog"] [role="alert"]')).not.toBeNull()
  })
})
