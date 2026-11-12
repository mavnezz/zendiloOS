import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { Job, VirtualMachine, VmBackup, VmPlugin } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatDateTime } from '@/shared/lib/format'
import { WINDOW_SECTION } from '@/shared/lib/window-section'
import { button, click, mountApp, page, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, reset } from '@/shared/test-support/server'

import VmsApp from './VmsApp.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)
vi.mock('@/shared/api/live', async () => ({
  live: (await import('@/shared/test-support/live')).live,
}))

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const PLUGIN: VmPlugin = {
  hardware: { kvm: true, cpu_virtualization: 'vmx', iommu: true, swtpm: true, secure_boot: true },
  installed: true,
  storage: { pool: 'tank', volume: 1 },
  locations: [
    { pool: 'tank', volume: 1 },
    { pool: 'cold', volume: 2 },
  ],
  pools: [],
  cores: 4,
  memory_bytes: 8 * 1024 ** 3,
  bridges: [],
  internal_networks: [],
  cards: [],
  image_upload_max_bytes: 32 * 1024 ** 3,
}

const ID = '0a1b2c3d4e5f'

function vm(extra: Partial<VirtualMachine> = {}): VirtualMachine {
  return {
    id: ID,
    name: 'office',
    os: 'windows',
    cpus: 2,
    memory_mib: 4096,
    firmware: 'uefi',
    disks: [{ slot: 0, size_gib: 64, bus: 'sata', pool: 'tank', volume: 1 }],
    nics: [],
    drives: [],
    boot: [{ kind: 'disk', slot: 0 }],
    autostart: false,
    usb: [],
    pci: [],
    tpm: true,
    secure_boot: true,
    created_at: 0,
    state: 'shut_off',
    ...extra,
  }
}

function backup(extra: Partial<VmBackup> = {}): VmBackup {
  return {
    id: 5,
    vm: ID,
    vm_exists: true,
    vm_name: 'office',
    target: { kind: 'share', share_id: 4 },
    place: 'backups',
    folder: `vm-backups/${ID}/20261112-030000`,
    taken_at: 1_793_000_000,
    consistency: 'crash',
    bytes: 3 * 1024 ** 3,
    files: [],
    os: 'windows',
    cpus: 2,
    memory_mib: 4096,
    disks_gib: [64],
    tpm: true,
    secure_boot: true,
    ...extra,
  }
}

function job(extra: Partial<Job> = {}): Job {
  return {
    id: 11,
    kind: 'vms.backup',
    target: 'office',
    state: 'running',
    step: 'backing_up',
    progress: 0.4,
    created_at: 0,
    updated_at: 0,
    output: [],
    ...extra,
  }
}

const section = ref<string | undefined>(undefined)

async function mountVms() {
  return mountApp(VmsApp, {
    global: {
      provide: {
        [WINDOW_SECTION as symbol]: {
          section,
          navigate: (next: string) => (section.value = next),
          openApp: vi.fn(),
        },
      },
    },
  })
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

beforeEach(() => {
  reset()
  section.value = undefined
  on('GET', '/api/v1/vms/plugin', PLUGIN)
  on('GET', '/api/v1/vms/images', [])
  on('GET', '/api/v1/vms', [vm()])
  on('GET', '/api/v1/vms/backups', [backup()])
  on('GET', `/api/v1/vms/${ID}/snapshots`, { snapshots: [], schedules: [] })
  on('GET', '/api/v1/files/shares', [
    { id: 3, name: 'media', access: 'read', home: false },
    { id: 4, name: 'backups', access: 'read_write', home: false },
  ])
  on('GET', '/api/v1/storage/removable', [
    {
      id: '1234-ABCD',
      label: 'STICK',
      device: 'sdf1',
      disk: 'sdf',
      filesystem: 'exfat',
      read_only: false,
      size_bytes: 64 * 1024 ** 3,
      state: 'mounted',
      path: '/run/zendiloos-removable/1234-ABCD',
    },
    {
      id: 'FFFF-0000',
      device: 'sdg1',
      disk: 'sdg',
      filesystem: 'fat',
      read_only: true,
      size_bytes: 1024 ** 3,
      state: 'mounted',
    },
  ])
  on('POST', `/api/v1/vms/${ID}/backups`, job())
  on('POST', `/api/v1/vms/${ID}/backups/cancel`, null)
  on('POST', '/api/v1/vms/backups/5/restore', {
    vm: '00000000000c',
    job: job({ id: 12, kind: 'vms.restore' }),
  })
  on('POST', '/api/v1/vms/backups/5/delete', null)
})

describe('backups and snapshots of VMs', () => {
  it('backs a VM up into a share from its card', async () => {
    await mountVms()
    await menu(t('vms.more', { name: 'office' }), t('vms.backup.action'))
    expect(page()).toContain(t('vms.backup.readable'))
    expect(document.querySelector<HTMLInputElement>('#vm-backup-folder')!.value).toBe(
      'backups/vm-backups',
    )
    expect(page()).toContain(t('vms.backup.into', { path: 'vm-backups', id: ID }))
    await type('#vm-backup-folder', 'media/x')
    expect(button(t('vms.backup.submit')).disabled).toBe(true)
    await type('#vm-backup-folder', 'backups/vms')
    await submit('#vm-backup')
    expect(called('POST', `/api/v1/vms/${ID}/backups`)[0]?.body).toEqual({
      target: { kind: 'share', share_id: 4 },
      folder: 'vms',
    })
    expect(useJobsStore().list.map((one) => one.id)).toContain(11)
  })

  it('backs a VM up onto a drive that can be written', async () => {
    await mountVms()
    await menu(t('vms.more', { name: 'office' }), t('vms.backup.action'))
    document.querySelector<HTMLInputElement>('input[name="vm-backup-to"][value="drive"]')!.click()
    await settle()
    const drives = [...document.querySelectorAll<HTMLOptionElement>('#vm-backup-drive option')]
    expect(drives.map((option) => option.value)).toEqual(['1234-ABCD'])
    await type('#vm-backup-drive-folder', 'a/../b')
    expect(page()).toContain(t('vms.backup.folderInvalid'))
    await type('#vm-backup-drive-folder', 'vm-backups')
    await submit('#vm-backup')
    expect(called('POST', `/api/v1/vms/${ID}/backups`)[0]?.body).toEqual({
      target: { kind: 'drive', drive: '1234-ABCD' },
      folder: 'vm-backups',
    })
  })

  it('shows a backup running on the card, and cancels it', async () => {
    await mountVms()
    useJobsStore().update(job())
    await settle()
    const work = document.querySelector('[data-testid="vm-work"]')?.textContent ?? ''
    expect(work).toContain(t('vms.work.backup'))
    expect(work).toContain(t('vms.jobSteps.backing_up'))
    expect(work).toContain('40')
    document
      .querySelector<HTMLButtonElement>(
        `button[aria-label="${t('vms.work.cancelLabel', { name: 'office' })}"]`,
      )!
      .click()
    await settle()
    expect(called('POST', `/api/v1/vms/${ID}/backups/cancel`)).toHaveLength(1)
    useJobsStore().update(job({ state: 'failed', error_code: 'canceled' }))
    await settle()
    expect(document.querySelector('[data-testid="vm-work"]')).toBeNull()
  })

  it('opens a VM’s snapshots from its card', async () => {
    await mountVms()
    await menu(t('vms.more', { name: 'office' }), t('vms.snapshots.action'))
    expect(document.querySelector('[role="dialog"]')?.textContent).toContain(
      t('vms.snapshots.title', { name: 'office' }),
    )
    expect(page()).toContain(t('vms.snapshots.empty'))
  })

  it('lists the backups and restores one as a new VM', async () => {
    section.value = 'backups'
    await mountVms()
    const row = document.querySelector('[data-backup="5"]')?.textContent ?? ''
    expect(row).toContain('office')
    expect(row).toContain(
      t('vms.backups.where', { place: 'backups', folder: `vm-backups/${ID}/20261112-030000` }),
    )
    expect(row).toContain(t('vms.snapshots.consistency.crash'))
    expect(row).toContain(t('vms.security.tpm'))
    expect(row).toContain('3 GiB')
    const more = t('vms.backups.more', { name: 'office' })

    await menu(more, t('vms.backups.restore'))
    expect(document.querySelector<HTMLInputElement>('#vm-restore-name')!.value).toBe(
      t('vms.backups.restoreDialog.nameSuffix', { name: 'office' }),
    )
    await type('#vm-restore-name', 'Office')
    expect(page()).toContain(t('vms.wizard.invalid.nameTaken'))
    await type('#vm-restore-name', 'office 2')
    await type('#vm-restore-pool', 'cold')
    await submit('#vm-restore')
    expect(called('POST', '/api/v1/vms/backups/5/restore')[0]?.body).toEqual({
      as: { kind: 'new', name: 'office 2', pool: 'cold' },
    })
    expect(useJobsStore().list.map((one) => one.id)).toContain(12)
  })

  it('restores a backup over the VM it came from, its name typed again', async () => {
    section.value = 'backups'
    await mountVms()
    await menu(t('vms.backups.more', { name: 'office' }), t('vms.backups.restore'))
    document.querySelector<HTMLInputElement>('input[name="vm-restore-how"][value="over"]')!.click()
    await settle()
    expect(page()).toContain(t('vms.backups.restoreDialog.overWarning'))
    expect(button(t('vms.backups.restoreDialog.submit')).disabled).toBe(true)
    await type('#vm-restore-confirm', 'office')
    await submit('#vm-restore')
    expect(called('POST', '/api/v1/vms/backups/5/restore')[0]?.body).toEqual({
      as: { kind: 'over', confirm: 'office' },
    })
  })

  it('deletes a backup after asking', async () => {
    section.value = 'backups'
    await mountVms()
    await menu(t('vms.backups.more', { name: 'office' }), t('vms.backups.delete'))
    expect(document.querySelector('[role="alertdialog"]')?.textContent).toContain(
      t('vms.backups.deleteTitle', { name: 'office', when: formatDateTime(1_793_000_000_000) }),
    )
    await click(t('vms.backups.delete'))
    expect(called('POST', '/api/v1/vms/backups/5/delete')).toHaveLength(1)
  })

  it('restores the backup of a VM that went only as a new one, and one that runs not over it', async () => {
    on('GET', '/api/v1/vms', [vm({ state: 'running' })])
    on('GET', '/api/v1/vms/backups', [
      backup({
        id: 6,
        vm: '00000000000f',
        vm_exists: false,
        vm_name: 'gone',
        tpm: false,
        secure_boot: false,
      }),
      backup(),
    ])
    section.value = 'backups'
    await mountVms()
    expect(document.querySelector('[data-backup="6"]')?.textContent).toContain(
      t('vms.backups.vmGone'),
    )
    await menu(t('vms.backups.more', { name: 'gone' }), t('vms.backups.restore'))
    expect(
      document.querySelector<HTMLInputElement>('input[name="vm-restore-how"][value="over"]')!
        .disabled,
    ).toBe(true)
    expect(page()).toContain(t('vms.backups.restoreDialog.overGone'))
    await click(t('desktop.cancel'))
    await menu(t('vms.backups.more', { name: 'office' }), t('vms.backups.restore'))
    document.querySelector<HTMLInputElement>('input[name="vm-restore-how"][value="over"]')!.click()
    await settle()
    expect(page()).toContain(t('vms.backups.restoreDialog.overRunning', { name: 'office' }))
    await type('#vm-restore-confirm', 'office')
    expect(button(t('vms.backups.restoreDialog.submit')).disabled).toBe(true)
  })

  it('says when there is no backup yet', async () => {
    on('GET', '/api/v1/vms/backups', [])
    section.value = 'backups'
    await mountVms()
    expect(page()).toContain(t('vms.backups.empty'))
    expect(document.querySelector('nav')?.textContent).toContain(t('vms.sections.backups'))
  })
})
