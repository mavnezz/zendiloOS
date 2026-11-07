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

import { useJobsStore } from '@/shared/jobs/store'
import { useNotifications } from '@/shared/lib/notifications'
import { account, group, job, share } from '@/shared/testing/fixtures'

import SharesManager from './SharesManager.vue'

const photos = share(10, 'photos', {
  description: 'Holidays',
  access: [
    { kind: 'account', id: 5, access: 'read_write' },
    { kind: 'group', id: 1, access: 'read' },
    { kind: 'account', id: 99, access: 'read' },
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

function serve(): void {
  answer('GET', '/api/v1/shares', [photos, empty])
  answer('GET', '/api/v1/accounts', [
    account(5, 'anna', { full_name: 'Anna A' }),
    account(6, 'ben'),
  ])
  answer('GET', '/api/v1/groups', [group(1, 'family')])
  answer('GET', '/api/v1/storage/pools', [
    { name: 'tank', volume: 1, mount_point: '/volume1' },
    { name: 'cold', volume: null, mount_point: null },
  ])
  answer('GET', '/api/v1/shares/10/snapshots', snapshots)
}

/** The section of the share named `name`. */
function section(name: string): HTMLElement {
  const found = [...document.body.querySelectorAll<HTMLElement>('section')].find(
    (s) => s.querySelector('h3')?.textContent === name,
  )
  if (!found) throw new Error(`no share ${name}`)
  return found
}

async function within(name: string, label: string): Promise<void> {
  section(name).querySelector<HTMLElement>(`[aria-label="${label}"]`)!.click()
  await settle()
}

async function show() {
  const wrapper = mountApp(SharesManager)
  await settle()
  return wrapper
}

function submit(selector: string): Promise<void> {
  document.body.querySelector<HTMLElement>(selector)!.closest('form')!.requestSubmit()
  return settle()
}

beforeEach(() => {
  cleanup()
  serve()
})

describe('the shares', () => {
  it('lists each share with its use, rules and services', async () => {
    await show()

    expect(section('photos').textContent).toContain('/volume1/photos · Holidays')
    expect(section('photos').textContent).toContain('1 GiB of 10 GiB used')
    expect(section('photos').textContent).toContain('NFS')
    const rules = section('photos').textContent.replace(/\s+/g, ' ')
    expect(rules).toContain('anna · Read and write')
    expect(rules).toContain('family (group) · Read')
    expect(section('photos').textContent).toContain('#99')
    expect(section('empty').textContent).toContain('Nobody has access yet.')
    expect(section('empty').textContent).not.toContain('Recycle bin')
  })

  it('says when there are none, or they cannot be read', async () => {
    answer('GET', '/api/v1/shares', [])
    await show()
    expect(page()).toContain('No shares yet.')

    cleanup()
    serve()
    on('GET', '/api/v1/shares', () => problem(500, 'internal'))
    await show()
    expect(document.body.querySelector('[role="alert"]')).not.toBeNull()
  })

  it('reads the shares again when a sharing job ends', async () => {
    await show()
    const before = callsTo('GET', '/api/v1/shares').length
    const jobs = useJobsStore()
    jobs.update(job(1, 'sharing.apply_access'))
    jobs.update(job(1, 'sharing.apply_access', { state: 'succeeded' }))
    await settle()
    expect(callsTo('GET', '/api/v1/shares').length).toBeGreaterThan(before)
  })

  it('deletes a share once its name is typed', async () => {
    on('POST', '/api/v1/shares/11/delete', () => job(3, 'sharing.delete_share'))
    on('POST', '/api/v1/shares/10/delete', () => problem(409, 'share_used_by_apps', 'jellyfin'))
    await show()

    await within('empty', 'Delete')
    expect(page()).toContain('Delete share empty?')
    const submitButton = () =>
      [...document.body.querySelectorAll<HTMLButtonElement>('button')].find(
        (b) => b.textContent?.trim() === 'Delete share and all files',
      )!
    expect(submitButton().disabled).toBe(true)
    await type('#share-confirm', 'empty')
    await submit('#share-confirm')
    expect(callsTo('POST', '/api/v1/shares/11/delete')[0]!.body).toEqual({ confirm: 'empty' })
    expect(useNotifications().items.some((n) => n.title === 'Share empty is being deleted')).toBe(
      true,
    )

    await within('photos', 'Delete')
    await type('#share-confirm', 'photos')
    await submit('#share-confirm')
    expect(page()).toContain('jellyfin')
    await click('Cancel')
    expect(page()).not.toContain('Delete share photos?')
  })
})

describe('the share dialog', () => {
  it('creates a share with rules, storage settings and NFS clients', async () => {
    on('POST', '/api/v1/shares', ({ body }) => share(12, (body as { name: string }).name))
    const wrapper = await show()

    await click('New share')
    expect(page()).toContain('/volume1 (tank)')
    expect(page()).not.toContain('(cold)')
    await type('#share-name', ' media ')
    await type('#share-description', 'Films')
    await choose('select[aria-label="family"]', 'read')
    await choose('select[aria-label="anna"]', 'read_write')
    await choose('select[aria-label="ben"]', 'deny')
    await tick('Recycle bin', false)
    await type('#share-quota', '2,5')
    await choose('#share-compression', 'off')
    await choose('#share-record-size', '16384')
    await click('Add computer or network')
    await click('Add computer or network')
    const networks = document.body.querySelectorAll<HTMLInputElement>(
      'input[aria-label^="IP address"]',
    )
    networks[0]!.value = '10.0.0.0/8'
    networks[0]!.dispatchEvent(new Event('input'))
    await tick('Write')
    // The second stays empty and is left out.
    await submit('#share-name')

    expect(callsTo('POST', '/api/v1/shares')[0]!.body).toEqual({
      name: 'media',
      description: 'Films',
      pool: 'tank',
      recycle_bin: false,
      access: [
        { kind: 'group', id: 1, access: 'read' },
        { kind: 'account', id: 5, access: 'read_write' },
        { kind: 'account', id: 6, access: 'deny' },
      ],
      nfs: [{ network: '10.0.0.0/8', write: true }],
      settings: {
        quota_bytes: Math.round(2.5 * 1024 ** 3),
        compression: 'off',
        record_size_bytes: 16384,
      },
    })
    expect(useNotifications().items.some((n) => n.title === 'Share media was created')).toBe(true)
    expect(wrapper.text()).toBeDefined()
  })

  it('edits a share from what it has, and says when files follow in the background', async () => {
    on('PATCH', '/api/v1/shares/10', () => ({ share: photos, job: job(4, 'sharing.apply_access') }))
    await show()

    await within('photos', 'Edit')
    expect(page()).toContain('Edit photos')
    expect(document.body.querySelector('#share-name')).toBeNull()
    expect(document.body.querySelector<HTMLInputElement>('#share-quota')!.value).toBe('10')
    expect(document.body.querySelector<HTMLSelectElement>('select[aria-label="anna"]')!.value).toBe(
      'read_write',
    )
    await press('Remove')
    await choose('select[aria-label="family"]', '')
    await type('#share-quota', '')
    await submit('#share-description')

    expect(callsTo('PATCH', '/api/v1/shares/10')[0]!.body).toMatchObject({
      description: 'Holidays',
      recycle_bin: true,
      nfs: [],
      settings: { quota_bytes: null, compression: 'zstd', record_size_bytes: 1048576 },
    })
    expect(useNotifications().items.find((n) => n.title === 'Changes to photos saved')?.body).toBe(
      'Existing files get the new permissions in the background.',
    )
  })

  it('shows a refusal and can be cancelled', async () => {
    on('POST', '/api/v1/shares', () => problem(409, 'name_taken'))
    await show()
    await click('New share')
    await type('#share-name', 'photos')
    await submit('#share-name')
    expect(page()).toContain('This name is already taken')
    await click('Cancel')
    expect(document.body.querySelector('[role="dialog"]')).toBeNull()
  })

  it('names the lack of volumes', async () => {
    answer('GET', '/api/v1/storage/pools', [])
    await show()
    await click('New share')
    expect(page()).toContain('There is no volume yet.')
  })
})

describe('the snapshots dialog', () => {
  it('shows snapshots and schedules, saves schedules and takes one by hand', async () => {
    on('PUT', '/api/v1/shares/10/snapshots/schedules', ({ body }) => body)
    on('POST', '/api/v1/shares/10/snapshots', () => snapshots.snapshots[0])
    await show()

    await within('photos', 'Snapshots')
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
    await show()
    await within('photos', 'Snapshots')

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

  it('rolls back once the share is named, warning of later snapshots', async () => {
    on('POST', /\/rollback$/, () => ({ destroyed: ['zdl-2026-11-07_10.00.00'] }))
    await show()
    await within('photos', 'Snapshots')

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
    on('GET', '/api/v1/shares/11/snapshots', () => problem(500, 'internal'))
    await show()
    await within('photos', 'Snapshots')
    await submit('#snapshot-note')
    expect(page()).toContain('A snapshot was taken this second already.')
    document.body
      .querySelector<HTMLButtonElement>('[role="dialog"] button[aria-label="Close"]')!
      .click()
    await settle()

    await within('empty', 'Snapshots')
    expect(document.body.querySelector('[role="dialog"] [role="alert"]')).not.toBeNull()
  })
})
