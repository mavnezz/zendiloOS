import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { VirtualMachine, VmImage, VmPlugin } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { button, click, mountApp, page, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import EditVmDialog from './EditVmDialog.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}, plural?: number) =>
  plural === undefined ? i18n.global.t(key, values) : i18n.global.t(key, values, plural)

const PLUGIN: VmPlugin = {
  hardware: {
    architecture: 'amd64',
    architecture_supported: true,
    kvm: true,
    cpu_virtualization: 'vmx',
    iommu: false,
    swtpm: true,
    secure_boot: true,
  },
  installed: true,
  storage: { pool: 'tank', volume: 1 },
  locations: [
    { pool: 'tank', volume: 1 },
    { pool: 'cold', volume: 2 },
  ],
  pools: [
    { pool: 'tank', volume: 1, health: 'online', used_bytes: 1 << 30, available_bytes: 99 << 30 },
  ],
  cores: 4,
  memory_bytes: 8 * 1024 ** 3,
  bridges: ['br0'],
  internal_networks: ['vmnet0'],
  cards: ['enp1s0', 'bond0'],
  image_upload_max_bytes: 32 * 1024 ** 3,
}

const IMAGES: VmImage[] = [
  {
    id: '00000000000a',
    name: 'debian-13.iso',
    bytes: 700 * 1024 ** 2,
    origin: { kind: 'share', share: 'media', path: 'isos/debian-13.iso' },
    added_at: 1,
    ready: true,
    used_by: [],
  },
  {
    id: '00000000000b',
    name: 'virtio-win.iso',
    bytes: null,
    origin: { kind: 'upload', file: 'virtio-win.iso' },
    added_at: 2,
    ready: false,
    used_by: [],
  },
]

function vm(extra: Partial<VirtualMachine> = {}): VirtualMachine {
  return {
    id: '0a1b2c3d4e5f',
    name: 'box',
    os: 'linux',
    cpus: 1,
    memory_mib: 1024,
    firmware: 'uefi',
    disks: [
      { slot: 0, size_gib: 20, bus: 'virtio', pool: 'tank', volume: 1 },
      { slot: 1, size_gib: 50, bus: 'sata', pool: 'tank', volume: 1 },
    ],
    nics: [{ slot: 0, model: 'virtio', network: { kind: 'user' }, mac: '52:54:00:12:34:56' }],
    drives: [{ slot: 0, image: null }],
    boot: [
      { kind: 'drive', slot: 0 },
      { kind: 'disk', slot: 0 },
    ],
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

async function mountEdit(machine: VirtualMachine = vm()) {
  return mountApp(EditVmDialog, {
    props: { open: true, vm: machine, plugin: PLUGIN, taken: ['web'], images: IMAGES },
  })
}

async function tab(id: string): Promise<void> {
  await click(t(`vms.edit.tabs.${id}`))
}

function value(selector: string): string {
  return document.querySelector<HTMLInputElement>(selector)!.value
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
  on('PUT', '/api/v1/vms/0a1b2c3d4e5f', vm())
})

describe('editing a VM', () => {
  it('changes the hardware of a VM that is off, warning about the firmware', async () => {
    await mountEdit()
    expect(document.querySelector('[data-testid="vm-edit-running"]')).toBeNull()
    await type('#vm-edit-name', 'web')
    expect(page()).toContain(t('vms.wizard.invalid.nameTaken'))
    await type('#vm-edit-name', ' box2 ')
    await tab('hardware')
    await type('#vm-edit-cpus', '2')
    await type('#vm-edit-memory', '2048')
    expect(document.querySelector('[data-testid="vm-firmware-warning"]')).toBeNull()
    document.querySelector<HTMLInputElement>('[data-firmware="bios"] input')!.click()
    await settle()
    expect(page()).toContain(t('vms.edit.firmwareWarning'))
    await tab('disks')
    expect(value('#vm-disk-0-size')).toBe('20')
    expect(page()).toContain(t('vms.devices.growOnly', { min: 20 }))
    await type('#vm-disk-0-size', '10')
    expect(button(t('vms.edit.save')).disabled).toBe(true)
    await type('#vm-disk-0-size', '30')
    // A disk the VM has stays where it is; a new one may go elsewhere.
    expect(document.querySelector<HTMLSelectElement>('#vm-disk-0-pool')!.disabled).toBe(true)
    await click(t('vms.devices.addDisk'))
    expect(document.querySelector<HTMLSelectElement>('#vm-disk-2-pool')!.disabled).toBe(false)
    await type('#vm-disk-2-pool', 'cold')
    await tab('network')
    expect(page()).toContain(t('vms.devices.mac', { mac: '52:54:00:12:34:56' }))
    await type('#vm-nic-0-network', 'bridge')
    await submit('#vm-edit')
    expect(called('PUT', '/api/v1/vms/0a1b2c3d4e5f')[0]?.body).toEqual({
      name: 'box2',
      cpus: 2,
      memory_mib: 2048,
      firmware: 'bios',
      disks: [
        { slot: 0, size_gib: 30, bus: 'virtio', pool: 'tank' },
        { slot: 1, size_gib: 50, bus: 'sata', pool: 'tank' },
        { slot: 2, size_gib: 32, bus: 'virtio', pool: 'cold' },
      ],
      nics: [{ slot: 0, model: 'virtio', network: { kind: 'bridge', bridge: 'br0' } }],
      drives: [{ slot: 0, image: null }],
      boot: [
        { kind: 'drive', slot: 0 },
        { kind: 'disk', slot: 0 },
      ],
      autostart: false,
      usb: [],
      pci: [],
      tpm: false,
      secure_boot: false,
      confirm: null,
    })
  })

  it('switches a TPM and Secure Boot only while the VM is off, saying the firmware starts afresh', async () => {
    await mountEdit()
    await tab('hardware')
    expect(document.querySelector('[data-testid="vm-fresh-variables"]')).toBeNull()
    document.querySelector<HTMLButtonElement>('#vm-tpm')!.click()
    await settle()
    document.querySelector<HTMLButtonElement>('#vm-secure-boot')!.click()
    await settle()
    expect(page()).toContain(t('vms.security.fresh'))
    await submit('#vm-edit')
    expect(called('PUT', '/api/v1/vms/0a1b2c3d4e5f')[0]?.body).toMatchObject({
      tpm: true,
      secure_boot: true,
    })
  })

  it('plugs USB devices in and out while the VM runs, and keeps its PCI devices', async () => {
    await mountEdit(
      vm({ state: 'running', usb: ['1234:5678'], pci: ['0000:01:00.0', '0000:01:00.1'] }),
    )
    await tab('hardware')
    expect(document.querySelector<HTMLButtonElement>('#vm-tpm')!.disabled).toBe(true)
    await tab('devices')
    // One it was given, unplugged since, stays offered.
    expect(page()).toContain(t('vms.hostDevices.notPlugged'))
    expect(document.querySelector<HTMLInputElement>('[data-group="1"] input')!.disabled).toBe(true)
    expect(page()).toContain(t('vms.hostDevices.pciRunning'))
    document.querySelector<HTMLInputElement>('[data-usb="046d:c31c"] input')!.click()
    await settle()
    document.querySelector<HTMLInputElement>('[data-usb="1234:5678"] input')!.click()
    await settle()
    expect(button(t('vms.edit.save')).disabled).toBe(false)
    await submit('#vm-edit')
    expect(called('PUT', '/api/v1/vms/0a1b2c3d4e5f')[0]?.body).toMatchObject({
      usb: ['046d:c31c'],
      pci: ['0000:01:00.0', '0000:01:00.1'],
    })
  })

  it('lets a PCI device the NAS no longer has go', async () => {
    await mountEdit(vm({ pci: ['0000:09:00.0'] }))
    await tab('devices')
    expect(page()).toContain(t('vms.hostDevices.gone'))
    document.querySelector<HTMLButtonElement>('[data-pci-gone="0000:09:00.0"] button')!.click()
    await settle()
    document.querySelector<HTMLInputElement>('[data-group="1"] input')!.click()
    await settle()
    await submit('#vm-edit')
    expect((called('PUT', '/api/v1/vms/0a1b2c3d4e5f')[0]?.body as { pci: string[] }).pci).toEqual([
      '0000:01:00.0',
      '0000:01:00.1',
    ])
  })

  it('removes a disk only with the name typed again', async () => {
    await mountEdit()
    await tab('disks')
    document
      .querySelector<HTMLButtonElement>(
        `button[aria-label="${t('vms.devices.removeDisk', { n: 2 })}"]`,
      )!
      .click()
    await settle()
    expect(page()).toContain(t('vms.edit.removing', { disks: '2' }, 1))
    expect(button(t('vms.edit.save')).disabled).toBe(true)
    await type('#vm-edit-confirm', 'Box')
    expect(button(t('vms.edit.save')).disabled).toBe(true)
    await type('#vm-edit-confirm', 'box')
    await submit('#vm-edit')
    const body = called('PUT', '/api/v1/vms/0a1b2c3d4e5f')[0]?.body as Record<string, unknown>
    expect(body.confirm).toBe('box')
    expect(body.disks).toEqual([{ slot: 0, size_gib: 20, bus: 'virtio', pool: 'tank' }])
  })

  it('changes only the name, autostart and media of a VM that runs', async () => {
    await mountEdit(vm({ state: 'running' }))
    expect(page()).toContain(t('vms.edit.running'))
    await tab('hardware')
    expect(document.querySelector<HTMLInputElement>('#vm-edit-cpus')!.disabled).toBe(true)
    await tab('disks')
    expect(button(t('vms.devices.addDisk')).disabled).toBe(true)
    await tab('media')
    expect(button(t('vms.devices.addDrive')).disabled).toBe(true)
    // An image still being added is offered but cannot be chosen.
    const options = [...document.querySelectorAll<HTMLOptionElement>('#vm-drive-0 option')]
    expect(options.map((option) => [option.textContent?.trim(), option.disabled])).toEqual([
      [t('vms.devices.noImage'), false],
      ['debian-13.iso', false],
      ['virtio-win.iso', true],
    ])
    await type('#vm-drive-0', '00000000000a')
    await tab('general')
    document.querySelector<HTMLButtonElement>('#vm-edit-autostart')!.click()
    await settle()
    await submit('#vm-edit')
    expect(called('PUT', '/api/v1/vms/0a1b2c3d4e5f')[0]?.body).toMatchObject({
      cpus: 1,
      autostart: true,
      drives: [{ slot: 0, image: '00000000000a' }],
    })
  })

  it('keeps the boot order to the devices there are, and says why the server refused', async () => {
    on('PUT', '/api/v1/vms/0a1b2c3d4e5f', problem(409, 'vm_running'))
    await mountEdit()
    await tab('media')
    const order = () =>
      [...document.querySelectorAll('[data-testid="vm-boot-order"] [data-boot]')].map((row) =>
        row.getAttribute('data-boot'),
      )
    expect(order()).toEqual(['drive:0', 'disk:0', 'disk:1'])
    document
      .querySelector<HTMLButtonElement>(
        `button[aria-label="${t('vms.devices.later', { device: t('vms.devices.drive', { n: 1 }) })}"]`,
      )!
      .click()
    await settle()
    expect(order()).toEqual(['disk:0', 'drive:0', 'disk:1'])
    await submit('#vm-edit')
    expect(page()).toContain(t('errors.vm_running'))
  })
})
