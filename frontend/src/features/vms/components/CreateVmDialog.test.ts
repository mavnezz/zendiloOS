import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { VmImage, VmPlugin } from '@/shared/api/types'
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
  hardware: { kvm: true, cpu_virtualization: 'vmx', iommu: false, swtpm: true, secure_boot: true },
  installed: true,
  storage: { pool: 'tank', volume: 1 },
  locations: [
    { pool: 'tank', volume: 1 },
    { pool: 'cold', volume: 2 },
  ],
  pools: [
    { pool: 'tank', volume: 1, health: 'online', used_bytes: 1 << 30, available_bytes: 99 << 30 },
  ],
  cores: 2,
  memory_bytes: 2 * 1024 ** 3,
  bridges: ['br0'],
  internal_networks: ['vmnet0'],
  cards: ['enp1s0', 'bond0'],
  image_upload_max_bytes: 32 * 1024 ** 3,
}

const DEBIAN: VmImage = {
  id: '00000000000a',
  name: 'debian-13.iso',
  bytes: 700 * 1024 ** 2,
  origin: { kind: 'share', share: 'media', path: 'isos/debian-13.iso' },
  added_at: 1,
  ready: true,
  used_by: [],
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

async function mountWizard(taken: string[] = [], images: VmImage[] = [DEBIAN]) {
  return mountApp(CreateVmDialog, { props: { open: true, plugin: PLUGIN, taken, images } })
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

const DEVICES = {
  iommu: { on: true, cmdline: ['intel_iommu=on'], cpu_vendor: 'intel' },
  usb: [{ id: '046d:c31c', port: '1-2', manufacturer: 'Logitech', name: 'USB Keyboard' }],
  pci: [
    {
      address: '0000:01:00.0',
      vendor: '10de',
      device: '1c82',
      class: '030000',
      vendor_name: 'NVIDIA Corporation',
      device_name: 'GP107',
      iommu_group: 1,
      driver: 'nouveau',
      needed: null,
    },
    {
      address: '0000:01:00.1',
      vendor: '10de',
      device: '0fb9',
      class: '040300',
      vendor_name: 'NVIDIA Corporation',
      device_name: 'GP107 Audio',
      iommu_group: 1,
      driver: null,
      needed: null,
    },
    {
      address: '0000:00:17.0',
      vendor: '8086',
      device: 'a352',
      class: '010601',
      vendor_name: 'Intel Corporation',
      device_name: 'SATA AHCI',
      iommu_group: 2,
      driver: 'ahci',
      needed: { kind: 'disk', disk: 'sda' },
    },
  ],
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/vms/devices', DEVICES)
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
    expect(currentStep()).toBe(stepLabel(3, 'storage'))
    expect(value('#vm-disk-0-size')).toBe('64')
    expect(value('#vm-disk-0-bus')).toBe('sata')
  })

  it('gives the VM disks, an image of the library and cards, and makes it', async () => {
    on('POST', '/api/v1/vms', { vm: {}, job: JOB })
    const { wrapper } = await mountWizard()
    await type('#vm-name', ' Debian ')
    await next()
    await type('#vm-cpus', '1')
    await type('#vm-memory', '256')
    await next()

    await type('#vm-disk-0-size', '1')
    await click(t('vms.devices.addDisk'))
    await type('#vm-disk-1-size', '0')
    expect(page()).toContain(t('vms.wizard.invalid.disk'))
    expect(button(t('vms.wizard.next')).disabled).toBe(true)
    await type('#vm-disk-1-size', '4')
    await type('#vm-disk-1-bus', 'sata')
    // A new disk goes on the first location unless another is chosen.
    expect(document.querySelector<HTMLSelectElement>('#vm-disk-0-pool')!.value).toBe('tank')
    await type('#vm-disk-1-pool', 'cold')
    await type('#vm-drive-0', DEBIAN.id)
    // Booted from the drive, then the first disk; the second boots too, last.
    document.querySelector<HTMLButtonElement>('[data-boot="disk:1"] [role="switch"]')!.click()
    await settle()
    await next()

    expect(currentStep()).toBe(stepLabel(4, 'review'))
    expect(page()).toContain('256 MiB')
    expect(page()).toContain('1 GiB, 4 GiB')
    expect(page()).toContain('debian-13.iso')
    await type('#vm-nic-0-network', 'bridge')
    expect(document.querySelector<HTMLSelectElement>('#vm-nic-0-device')!.value).toBe('br0')
    await click(t('vms.devices.addNic'))
    await type('#vm-nic-1-model', 'e1000e')
    await submit('#vm-wizard')
    expect(called('POST', '/api/v1/vms')[0]?.body).toEqual({
      name: 'Debian',
      os: 'linux',
      cpus: 1,
      memory_mib: 256,
      firmware: 'uefi',
      disks: [
        { slot: 0, size_gib: 1, bus: 'virtio', pool: null },
        { slot: 1, size_gib: 4, bus: 'sata', pool: 'cold' },
      ],
      nics: [
        { slot: 0, model: 'virtio', network: { kind: 'bridge', bridge: 'br0' } },
        { slot: 1, model: 'e1000e', network: { kind: 'user' } },
      ],
      drives: [{ slot: 0, image: DEBIAN.id }],
      boot: [
        { kind: 'drive', slot: 0 },
        { kind: 'disk', slot: 0 },
        { kind: 'disk', slot: 1 },
      ],
      autostart: false,
      usb: [],
      pci: [],
      tpm: false,
      secure_boot: false,
    })
    expect(useJobsStore().list.map((job) => job.id)).toContain(7)
    expect(wrapper.emitted('created')).toHaveLength(1)
  })

  it('puts cards directly on a card and on an internal network', async () => {
    on('POST', '/api/v1/vms', { vm: {}, job: JOB })
    await mountWizard()
    await type('#vm-name', 'net')
    for (let step = 0; step < 3; step++) await next()
    await type('#vm-nic-0-network', 'internal')
    expect(document.querySelector<HTMLSelectElement>('#vm-nic-0-device')!.value).toBe('vmnet0')
    await click(t('vms.devices.addNic'))
    await type('#vm-nic-1-network', 'direct')
    await type('#vm-nic-1-device', 'bond0')
    expect(page()).toContain(t('vms.networks.means.direct'))
    await submit('#vm-wizard')
    expect((called('POST', '/api/v1/vms')[0]?.body as { nics: unknown }).nics).toEqual([
      { slot: 0, model: 'virtio', network: { kind: 'internal', network: 'vmnet0' } },
      { slot: 1, model: 'virtio', network: { kind: 'direct', card: 'bond0' } },
    ])
  })

  it('gives Windows a TPM and Secure Boot, which BIOS cannot have, and the NAS’s devices', async () => {
    on('POST', '/api/v1/vms', { vm: {}, job: JOB })
    await mountWizard()
    await type('#vm-name', 'Windows 11')
    radio('data-os', 'windows').dispatchEvent(new Event('change'))
    await settle()
    await next()
    const tpm = document.querySelector<HTMLButtonElement>('#vm-tpm')!
    const secure = document.querySelector<HTMLButtonElement>('#vm-secure-boot')!
    expect([tpm.getAttribute('aria-checked'), secure.getAttribute('aria-checked')]).toEqual([
      'true',
      'true',
    ])
    radio('data-firmware', 'bios').dispatchEvent(new Event('change'))
    await settle()
    expect(secure.getAttribute('aria-checked')).toBe('false')
    expect(secure.disabled).toBe(true)
    expect(page()).toContain(t('vms.security.secureBootBios'))
    radio('data-firmware', 'uefi').dispatchEvent(new Event('change'))
    await settle()
    secure.click()
    await settle()
    await next()
    await next()
    // The keyboard, and the graphics card with its sound; the disks' controller stays.
    document.querySelector<HTMLInputElement>('[data-usb="046d:c31c"] input')!.click()
    await settle()
    document.querySelector<HTMLInputElement>('[data-group="1"] input')!.click()
    await settle()
    expect(document.querySelector<HTMLInputElement>('[data-group="2"] input')!.disabled).toBe(true)
    expect(page()).toContain(t('vms.hostDevices.needs.disk', { disk: 'sda' }))
    expect(page()).toContain(t('vms.hostDevices.pciWarning'))
    expect(page()).toContain(t('vms.hostDevices.summary', { usb: 1, pci: 2 }))
    await submit('#vm-wizard')
    expect(called('POST', '/api/v1/vms')[0]?.body).toMatchObject({
      os: 'windows',
      firmware: 'uefi',
      tpm: true,
      secure_boot: true,
      usb: ['046d:c31c'],
      pci: ['0000:01:00.0', '0000:01:00.1'],
    })
  })

  it('says how to switch the IOMMU on where it is off', async () => {
    on('GET', '/api/v1/vms/devices', {
      ...DEVICES,
      iommu: { on: false, cmdline: [], cpu_vendor: 'amd' },
    })
    await mountWizard()
    await type('#vm-name', 'box')
    for (let step = 0; step < 3; step++) await next()
    expect(page()).toContain(t('vms.hostDevices.iommuOff'))
    expect(page()).toContain(t('vms.hostDevices.iommuHint.amd'))
    expect(document.querySelector('[data-group="1"]')).toBeNull()
  })

  it('leads to the library while it has no image', async () => {
    const { wrapper } = await mountWizard([], [])
    await type('#vm-name', 'box')
    await next()
    await next()
    expect(page()).toContain(t('vms.devices.noImages'))
    await click(t('vms.devices.toImages'))
    expect(wrapper.emitted('library')).toHaveLength(1)
  })

  it('says why the server refused', async () => {
    on('POST', '/api/v1/vms', problem(409, 'vms_not_set_up'))
    await mountWizard()
    await type('#vm-name', 'box')
    for (let step = 0; step < 3; step++) await next()
    await submit('#vm-wizard')
    expect(page()).toContain(t('errors.vms_not_set_up'))
  })

  /** Without a bridge, the wizard says where one is made. */
  it('points to the Network page while the NAS has no bridge', async () => {
    const openApp = vi.fn()
    await mountApp(CreateVmDialog, {
      props: { open: true, plugin: { ...PLUGIN, bridges: [] }, taken: [], images: [] },
      global: {
        provide: {
          [WINDOW_SECTION as symbol]: { section: ref(undefined), navigate: vi.fn(), openApp },
        },
      },
    })
    await type('#vm-name', 'box')
    for (let step = 0; step < 3; step++) await next()
    expect(page()).toContain(t('vms.wizard.noBridge'))
    // On a bridge there is none to choose, and the wizard says so before creating.
    await type('#vm-nic-0-network', 'bridge')
    const bridges = [...document.querySelectorAll<HTMLOptionElement>('#vm-nic-0-device option')]
    expect(bridges.map((option) => option.value)).toEqual([''])
    expect(page()).toContain(t('vms.wizard.invalid.network'))
    expect(button(t('vms.wizard.submit')).disabled).toBe(true)
    await click(t('vms.wizard.toNetwork'))
    expect(openApp).toHaveBeenCalledWith('settings', 'network')
  })
})
