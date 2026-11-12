import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { VirtualMachine, VmSnapshot } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { button, click, mountApp, page, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import SnapshotsDialog from './SnapshotsDialog.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}, plural?: number) =>
  plural === undefined ? i18n.global.t(key, values) : i18n.global.t(key, values, plural)

const ID = '0a1b2c3d4e5f'
const PATH = `/api/v1/vms/${ID}/snapshots`

function vm(extra: Partial<VirtualMachine> = {}): VirtualMachine {
  return {
    id: ID,
    name: 'web',
    os: 'linux',
    cpus: 1,
    memory_mib: 1024,
    firmware: 'uefi',
    disks: [{ slot: 0, size_gib: 20, bus: 'virtio', pool: 'tank', volume: 1 }],
    nics: [],
    drives: [],
    boot: [{ kind: 'disk', slot: 0 }],
    autostart: false,
    usb: [],
    pci: [],
    tpm: false,
    secure_boot: false,
    created_at: 0,
    state: 'shut_off',
    ...extra,
  }
}

function snapshot(label: string, takenAt: number, extra: Partial<VmSnapshot> = {}): VmSnapshot {
  return {
    label,
    taken_at: takenAt,
    name: null,
    schedules: [],
    consistency: 'off',
    disks: [0, 1],
    used_bytes: 4096,
    referenced_bytes: 8192,
    ...extra,
  }
}

const NEWER = snapshot('zdl-2026-11-12_04.00.00', 1_793_000_000 + 3600, {
  schedules: ['hourly'],
  consistency: 'crash',
})
const OLDER = snapshot('zdl-2026-11-12_03.00.00', 1_793_000_000, { name: 'before the update' })

async function mountSnapshots(machine: VirtualMachine = vm()) {
  return mountApp(SnapshotsDialog, { props: { open: true, vm: machine } })
}

/** Opens a reka menu the way a pointer does, and picks `item`. */
async function menu(label: string, item: string): Promise<void> {
  const trigger = document.querySelector(`button[aria-label="${label}"]`)!
  for (const kind of ['pointerdown', 'pointerup', 'click'])
    trigger.dispatchEvent(new MouseEvent(kind, { bubbles: true, button: 0 }))
  await settle()
  const entry = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((element) =>
    element.textContent?.includes(item),
  )
  if (!entry) throw new Error(`no menu item ${item}`)
  entry.click()
  await settle()
}

const more = (snapshot: VmSnapshot) =>
  t('vms.snapshots.more', { when: formatDateTime(snapshot.taken_at * 1000) })

beforeEach(() => {
  reset()
  on('GET', PATH, { snapshots: [NEWER, OLDER], schedules: [{ frequency: 'hourly', keep: 24 }] })
  on('POST', PATH, OLDER)
  on('POST', `${PATH}/${OLDER.label}/rollback`, { removed: [NEWER.label] })
  on('POST', `${PATH}/${NEWER.label}/rollback`, { removed: [] })
  on('POST', `${PATH}/${OLDER.label}/delete`, null)
  on('PUT', `${PATH}/schedules`, [])
})

describe('the snapshots of a VM', () => {
  it('lists them newest first with how consistent they are, and takes one with a name', async () => {
    await mountSnapshots()
    const rows = [...document.querySelectorAll('[data-snapshot]')]
    expect(rows.map((row) => row.getAttribute('data-snapshot'))).toEqual([NEWER.label, OLDER.label])
    expect(rows[0]?.textContent).toContain(t('vms.snapshots.unnamed'))
    expect(rows[0]?.textContent).toContain(t('vms.snapshots.consistency.crash'))
    expect(rows[0]?.textContent).toContain(t('vms.snapshots.frequencies.hourly'))
    expect(rows[1]?.textContent).toContain('before the update')
    expect(rows[1]?.textContent).toContain(t('vms.snapshots.byHand'))
    expect(rows[1]?.textContent).toContain(t('vms.snapshots.disks', { disks: '1, 2' }, 2))
    expect(page()).toContain(t('vms.snapshots.consistencyHint.crash'))
    expect(document.querySelector('[data-testid="vm-snapshot-running"]')).toBeNull()
    await type('#vm-snapshot-name', 'x'.repeat(65))
    expect(button(t('vms.snapshots.take')).disabled).toBe(true)
    await type('#vm-snapshot-name', ' before the update ')
    await submit('#vm-snapshot-take')
    expect(called('POST', PATH)[0]?.body).toEqual({ name: 'before the update' })
  })

  it('rolls back to an older one only naming the later ones that go', async () => {
    await mountSnapshots()
    await menu(more(OLDER), t('vms.snapshots.rollback'))
    const dialog = document.querySelector('[role="alertdialog"]')?.textContent ?? ''
    expect(dialog).toContain(t('vms.snapshots.rollbackNewer', { labels: NEWER.label }, 1))
    await click(t('vms.snapshots.rollback'))
    expect(called('POST', `${PATH}/${OLDER.label}/rollback`)[0]?.body).toEqual({ newer: true })
  })

  it('rolls back to the newest without more ado', async () => {
    await mountSnapshots()
    await menu(more(NEWER), t('vms.snapshots.rollback'))
    await click(t('vms.snapshots.rollback'))
    expect(called('POST', `${PATH}/${NEWER.label}/rollback`)[0]?.body).toEqual({ newer: false })
  })

  it('rolls back only while the VM is off, and says it runs', async () => {
    await mountSnapshots(vm({ state: 'running' }))
    expect(page()).toContain(t('vms.snapshots.running'))
    const trigger = document.querySelector(`button[aria-label="${more(OLDER)}"]`)!
    for (const kind of ['pointerdown', 'pointerup', 'click'])
      trigger.dispatchEvent(new MouseEvent(kind, { bubbles: true, button: 0 }))
    await settle()
    const entry = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((element) =>
      element.textContent?.includes(t('vms.snapshots.rollbackOff')),
    )
    expect(
      entry?.getAttribute('aria-disabled') ?? entry?.getAttribute('data-disabled'),
    ).not.toBeNull()
  })

  it('deletes one and says why the server refused', async () => {
    on('POST', `${PATH}/${OLDER.label}/delete`, problem(409, 'refused', 'busy'))
    await mountSnapshots()
    await menu(more(OLDER), t('vms.snapshots.delete'))
    await click(t('vms.snapshots.delete'))
    expect(called('POST', `${PATH}/${OLDER.label}/delete`)).toHaveLength(1)
    expect(page()).toContain('(busy)')
  })

  it('keeps schedules of 1 to 100 each', async () => {
    await mountSnapshots()
    expect(document.querySelector<HTMLInputElement>('#vm-keep-hourly')!.value).toBe('24')
    document
      .querySelector<HTMLInputElement>('[data-frequency="daily"] input[type="checkbox"]')!
      .click()
    await settle()
    expect(document.querySelector<HTMLInputElement>('#vm-keep-daily')!.value).toBe('7')
    await type('#vm-keep-daily', '0')
    expect(page()).toContain(t('vms.snapshots.keepInvalid'))
    expect(button(t('vms.snapshots.saveSchedules')).disabled).toBe(true)
    await type('#vm-keep-daily', '14')
    document
      .querySelector<HTMLInputElement>('[data-frequency="hourly"] input[type="checkbox"]')!
      .click()
    await settle()
    await submit('#vm-snapshot-schedules')
    expect(called('PUT', `${PATH}/schedules`)[0]?.body).toEqual({
      schedules: [{ frequency: 'daily', keep: 14 }],
    })
    expect(page()).toContain(t('vms.snapshots.saved'))
  })

  it('says when there is none yet', async () => {
    on('GET', PATH, { snapshots: [], schedules: [] })
    await mountSnapshots()
    expect(page()).toContain(t('vms.snapshots.empty'))
  })
})
