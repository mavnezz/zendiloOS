import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { VmPlugin } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatBytes } from '@/shared/lib/format'
import { button, click, mountApp, page, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import { WINDOW_SECTION } from '@/shared/lib/window-section'

import CreateVmDialog from './CreateVmDialog.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> | number = {}) =>
  typeof values === 'number' ? i18n.global.t(key, values) : i18n.global.t(key, values)

const PLUGIN: VmPlugin = {
  hardware: { kvm: true, cpu_virtualization: 'vmx', iommu: false },
  installed: true,
  storage: { pool: 'tank', volume: 1 },
  pools: [{ pool: 'tank', volume: 1 }],
  cores: 2,
  memory_bytes: 2 * 1024 ** 3,
  bridges: ['br0'],
}

const JOB = {
  id: 7,
  kind: 'vms.create',
  target: 'web',
  state: 'running',
  step: null,
  progress: null,
  error_code: null,
  error_detail: null,
  created_at: 0,
  updated_at: 0,
  finished_at: null,
  output: [],
}

async function mountWizard(taken: string[] = []) {
  return mountApp(CreateVmDialog, { props: { open: true, plugin: PLUGIN, taken } })
}

/** The step the step bar marks current. */
function currentStep(): string {
  return document.querySelector('[aria-current="step"]')?.textContent?.replace(/\s+/g, '') ?? ''
}

/** A step as the step bar shows it: its number and its name, without spaces. */
function stepLabel(number: number, name: string): string {
  return `${number}${t(`vms.wizard.steps.${name}`)}`.replace(/\s+/g, '')
}

function radio(attribute: string, value: string): HTMLInputElement {
  return document.querySelector<HTMLInputElement>(`[${attribute}="${value}"] input`)!
}

function value(selector: string): string {
  return document.querySelector<HTMLInputElement>(selector)!.value
}

async function next(): Promise<void> {
  await click(t('vms.wizard.next'))
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/files/shares', [
    { id: 3, name: 'media', access: 'read', home: false },
    { id: 4, name: 'docs', access: 'read_write', home: false },
  ])
  on('GET', '/api/v1/files/shares/3/folder', (call: { query: Record<string, string> }) => ({
    entries:
      call.query.path === 'isos'
        ? [
            { name: 'debian-13.iso', kind: 'file' },
            { name: 'notes.txt', kind: 'file' },
          ]
        : [{ name: 'isos', kind: 'directory' }],
    writable: false,
  }))
})

describe('the wizard for a new VM', () => {
  it('needs a name before it goes on, and no other VM may have it', async () => {
    await mountWizard(['Web'])
    expect(currentStep()).toBe(stepLabel(1, 'system'))
    expect(button(t('vms.wizard.next')).disabled).toBe(true)
    await type('#vm-name', 'web')
    expect(page()).toContain(t('vms.wizard.invalid.nameTaken'))
    expect(button(t('vms.wizard.next')).disabled).toBe(true)
    await type('#vm-name', 'Debian server')
    expect(button(t('vms.wizard.next')).disabled).toBe(false)
  })

  it('fills in what a preset needs, held to the machine', async () => {
    await mountWizard()
    await type('#vm-name', 'Office')
    radio('data-os', 'windows').dispatchEvent(new Event('change'))
    await settle()
    await next()
    expect(currentStep()).toBe(stepLabel(2, 'resources'))
    // The labels name what the NAS has.
    expect(page()).toContain(t('vms.wizard.cpus', { n: 2 }))
    expect(page()).toContain(t('vms.wizard.memory', { total: formatBytes(2 * 1024 ** 3) }))
    expect(value('#vm-cpus')).toBe('2')
    // Windows asks for 4 GiB; this NAS has 2, of which a VM is offered half.
    expect(value('#vm-memory')).toBe('1024')
    expect(radio('data-firmware', 'uefi').checked).toBe(true)
    await type('#vm-cpus', '3')
    expect(page()).toContain(t('vms.wizard.invalid.cpus', { max: 2 }))
    expect(button(t('vms.wizard.next')).disabled).toBe(true)
    await type('#vm-cpus', '1')
    await next()
    expect(value('#vm-disk')).toBe('64')
  })

  it('picks an ISO from a share and makes the VM', async () => {
    on('POST', '/api/v1/vms', { vm: {}, job: JOB })
    const { wrapper } = await mountWizard()
    await type('#vm-name', ' Debian ')
    await next()
    await type('#vm-cpus', '1')
    await type('#vm-memory', '256')
    await next()
    await type('#vm-disk', '1')

    await click(t('vms.wizard.pickIso'))
    const tree = () => [...document.querySelectorAll('[role="treeitem"]')]
    const expand = (name: string) =>
      tree()
        .find((row) => row.textContent?.trim() === name)!
        .querySelector('button')!
        .click()
    expand('media')
    await settle()
    expand('isos')
    await settle()
    expect(page()).toContain('debian-13.iso')
    expect(page()).not.toContain('notes.txt')
    // A folder is no ISO.
    const pickedFolder = tree().find((row) => row.textContent?.trim() === 'isos')!
    pickedFolder.querySelectorAll('button')[1]!.click()
    await settle()
    expect(button(t('folderPicker.confirm')).disabled).toBe(true)
    const file = tree().find((row) => row.textContent?.trim() === 'debian-13.iso')!
    file.querySelectorAll('button')[1]!.click()
    await settle()
    await click(t('folderPicker.confirm'))
    expect(document.querySelector('[data-testid="vm-iso"]')?.textContent?.trim()).toBe(
      'media/isos/debian-13.iso',
    )
    await next()

    expect(currentStep()).toBe(stepLabel(4, 'review'))
    expect(page()).toContain(t('vms.wizard.bridge', { bridge: 'br0' }))
    expect(page()).toContain('256 MiB')
    radio('data-network', 'br0').click()
    await settle()
    await submit('#vm-wizard')
    expect(called('POST', '/api/v1/vms')[0]?.body).toEqual({
      name: 'Debian',
      os: 'linux',
      cpus: 1,
      memory_mib: 256,
      firmware: 'uefi',
      disk_gib: 1,
      network: { kind: 'bridge', bridge: 'br0' },
      autostart: false,
      iso: { share_id: 3, path: 'isos/debian-13.iso' },
    })
    expect(useJobsStore().list.map((job) => job.id)).toContain(7)
    expect(wrapper.emitted('created')).toHaveLength(1)
  })

  it('says why the server refused', async () => {
    on('POST', '/api/v1/vms', problem(422, 'iso_denied'))
    await mountWizard()
    await type('#vm-name', 'box')
    for (let step = 0; step < 3; step++) await next()
    await submit('#vm-wizard')
    expect(page()).toContain(t('errors.iso_denied'))
  })

  /** Without a bridge, the wizard says where one is made. */
  it('points to the Network page while the NAS has no bridge', async () => {
    const openApp = vi.fn()
    await mountApp(CreateVmDialog, {
      props: { open: true, plugin: { ...PLUGIN, bridges: [] }, taken: [] },
      global: {
        provide: {
          [WINDOW_SECTION as symbol]: { section: ref(undefined), navigate: vi.fn(), openApp },
        },
      },
    })
    await type('#vm-name', 'box')
    for (let step = 0; step < 3; step++) await next()
    expect(page()).toContain(t('vms.wizard.noBridge'))
    expect(document.querySelector('[data-network="br0"]')).toBeNull()
    await click(t('vms.wizard.toNetwork'))
    expect(openApp).toHaveBeenCalledWith('settings', 'network')
  })
})
