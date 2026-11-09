import {
  answer,
  buttons,
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
  answer('GET', '/api/v1/groups', [group(1, 'family', [6])])
  answer('GET', '/api/v1/shares/10/snapshots', snapshots)
})

const counted = {
  path: '',
  folder: true,
  bytes: 2048,
  files: 1234,
  folders: 3,
  hidden: 1,
  stopped: null,
  created: 1_690_000_000,
  modified: 1_700_000_100,
  owner: 'anna',
}

async function properties(of = photos, tab?: 'general' | 'permissions' | 'sharing' | 'nfs') {
  await mountApp(FolderPropertiesDialog, { props: { share: of, tab, open: false } }).setProps({
    open: true,
  })
  await settle()
}

async function snapshotsOf(of = photos) {
  await mountApp(SnapshotsDialog, { props: { share: of, open: false } }).setProps({ open: true })
  await settle()
}

function box(label: string): HTMLInputElement {
  const found = document.body.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)
  if (!found) throw new Error(`no checkbox ${label}`)
  return found
}
async function toggle(label: string): Promise<void> {
  box(label).click()
  await settle()
}
/** The row of the permissions table naming `name`. */
function rowOf(name: string): string {
  const row = [...document.body.querySelectorAll('tbody tr')].find((r) =>
    r.querySelector('td')?.textContent?.trim().startsWith(name),
  )
  return row?.textContent ?? ''
}

describe('the properties of a folder on a volume', () => {
  it('tell where it is, how to reach it and what it holds, counted on the NAS', async () => {
    answer('GET', '/api/v1/files/shares/10/facts', counted)
    await properties({ ...photos, shared: true })
    expect(page()).toContain('/volume1/photos')
    expect(page()).toContain(`\\${window.location.hostname}\\photos`)
    expect(page()).toContain(`${window.location.hostname}:/volume1/photos`)
    expect(page()).toContain('1,234 files and 3 folders holding 2 KiB; 1 hidden')
    expect(page()).toContain('anna')
    expect(callsTo('GET', '/api/v1/files/shares/10/facts')[0]!.query.get('path')).toBe('')
  })

  it('say a count is short, and count nothing when the NAS cannot', async () => {
    let release: (value: unknown) => void = () => {}
    on('GET', '/api/v1/files/shares/10/facts', () => new Promise((done) => (release = done)))
    await properties()
    expect(page()).toContain('Counting…')
    release({ ...counted, stopped: 'time' })
    await settle()
    expect(page()).toContain('Counted so far: counting took longer than half a minute.')
    cleanup()
    answer('GET', '/api/v1/accounts', [])
    answer('GET', '/api/v1/groups', [])
    await properties()
    expect(page()).toContain('Could not be counted.')
  })

  it('change storage, permissions, sharing and NFS, which hold at once', async () => {
    on('PATCH', '/api/v1/shares/10', () => photos)
    answer('GET', '/api/v1/files/shares/10/facts', counted)
    await properties()
    expect(page()).toContain('Properties of photos')
    expect(document.body.querySelector<HTMLInputElement>('#share-quota')!.value).toBe('10')
    await type('#share-quota', '')
    await click('Permissions')
    expect(box('anna: Read and write').checked).toBe(true)
    // ben reads through family.
    expect(rowOf('ben')).toContain('Read')
    await press('Members of family')
    expect(page()).toContain('One member')
    await toggle('family: Read')
    expect(rowOf('ben')).toContain('No access')
    await toggle('ben: Read')
    await toggle('ben: Deny access')
    expect(box('ben: Read').checked).toBe(false)
    expect(rowOf('ben')).toContain('Deny access')
    await click('Sharing')
    await tick('Share on the network', false)
    await click('NFS')
    expect(page()).toContain('NFS offers it only once it is shared again')
    await press('Remove the rule for 192.168.1.0/24')
    expect(page()).toContain('No computer may mount this folder over NFS.')
    await click('Add a rule')
    await type('#nfs-client', ' 10.0.0.0/8 ')
    await choose('#nfs-permission', 'true')
    expect(page()).toContain('every request counts as the group zendiloos-files')
    expect(document.body.querySelector('#nfs-squash')).toBeNull()
    await tick('Allow connections from ports above 1024')
    await click('OK')
    await click('Save')

    expect(callsTo('PATCH', '/api/v1/shares/10')[0]!.body).toEqual({
      description: 'Holidays',
      shared: false,
      recycle_bin: true,
      access: [
        { kind: 'account', id: 5, access: 'read_write' },
        { kind: 'account', id: 6, access: 'deny' },
      ],
      nfs: [{ network: '10.0.0.0/8', write: true, async: false, insecure: true }],
      settings: { quota_bytes: null, compression: 'zstd', record_size_bytes: 1048576 },
    })
    const saved = useNotifications().items.find((n) => n.title === 'Changes to photos saved')
    expect(saved).toBeDefined()
    expect(saved?.body).toBeUndefined()
  })

  it('check NFS rules in their dialog, and change one', async () => {
    on('PATCH', '/api/v1/shares/10', () => photos)
    await properties(photos, 'nfs')
    expect(page()).toContain('NFS tells computers apart, not people')
    await click('Add a rule')
    await type('#nfs-client', 'nas lan')
    expect(page()).toContain('Not an address, network, host name or pattern.')
    expect(buttons('OK')[0]!.disabled).toBe(true)
    await type('#nfs-client', '192.168.1.0/24')
    expect(page()).toContain('There is a rule for it already.')
    buttons('Cancel').at(-1)!.click()
    await settle()
    await press('Change the rule for 192.168.1.0/24')
    expect(document.body.querySelector<HTMLInputElement>('#nfs-client')!.value).toBe(
      '192.168.1.0/24',
    )
    await tick('Answer writes before they are on disk')
    await type('#nfs-client', '*.Example.com')
    await click('OK')
    expect(page()).toContain('*.example.com')
    await click('Save')
    expect(callsTo('PATCH', '/api/v1/shares/10')[0]!.body).toMatchObject({
      nfs: [{ network: '*.example.com', write: false, async: true, insecure: false }],
    })
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
    // A snapshot from before the walk to share-level access brings that walk back, as a job.
    on('POST', /\/rollback$/, () => ({
      destroyed: ['zdl-2026-11-07_10.00.00'],
      job: job(9, 'sharing.migrate_permissions', { target: 'photos' }),
    }))
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
    expect(page()).not.toContain('permissions')

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
