import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'

import type { VmHostPci, VmPlugin } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { mountApp, page, settle } from '@/shared/test-support/mount'
import { on, reset } from '@/shared/test-support/server'

import { presetForm, type VmForm } from '../lib/vms'
import DevicesEditor from './DevicesEditor.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const PLUGIN = {
  hardware: {
    architecture: 'amd64',
    architecture_supported: true,
    kvm: true,
    cpu_virtualization: 'vmx',
    iommu: true,
    swtpm: true,
    secure_boot: true,
  },
  cores: 4,
  memory_bytes: 8 * 1024 ** 3,
} as VmPlugin

function pci(
  address: string,
  cls: string,
  group: number | null,
  extra: Partial<VmHostPci> = {},
): VmHostPci {
  return {
    address,
    vendor: '8086',
    device: '1533',
    class: cls,
    vendor_name: null,
    device_name: null,
    iommu_group: group,
    driver: null,
    needed: null,
    ...extra,
  }
}

const form = ref<VmForm>(presetForm('linux', PLUGIN, 'box'))

/** The editor with a form of its own, as a dialog has it. */
const Host = defineComponent({
  setup: () => () =>
    h(DevicesEditor, {
      form: form.value,
      'onUpdate:form': (next: VmForm) => (form.value = next),
    }),
})

beforeEach(() => {
  reset()
  form.value = presetForm('linux', PLUGIN, 'box')
  on('GET', '/api/v1/vms/devices', {
    iommu: { on: true, cmdline: [], cpu_vendor: null },
    usb: [
      { id: '0781:5583', port: '1-3', manufacturer: null, name: null },
      { id: '0781:5583', port: '1-4', manufacturer: null, name: null },
    ],
    pci: [
      pci('0000:00:02.0', '030000', 1, { needed: { kind: 'console' } }),
      pci('0000:00:03.0', '020000', 2, { needed: { kind: 'network', interface: 'enp0s3' } }),
      pci('0000:00:04.0', '020000', 3, { needed: { kind: 'network', interface: null } }),
      pci('0000:00:00.0', '060000', 4, { needed: { kind: 'bridge' } }),
      pci('0000:05:00.0', '0c0330', null),
      pci('0000:06:00.0', '020000', 6, { driver: 'igb' }),
    ],
  })
})

describe('the NAS’s devices for a VM', () => {
  it('says why each device the NAS needs stays with it', async () => {
    await mountApp(Host)
    const blocked = [...document.querySelectorAll('[data-testid="vm-pci-blocked"]')].map(
      (element) => element.textContent ?? '',
    )
    expect(blocked.join('|')).toContain(t('vms.hostDevices.needs.console'))
    expect(blocked.join('|')).toContain(t('vms.hostDevices.needs.network', { interface: 'enp0s3' }))
    expect(blocked.join('|')).toContain(t('vms.hostDevices.needs.networkUnknown'))
    expect(blocked.join('|')).toContain(t('vms.hostDevices.needs.bridge'))
    expect(blocked.join('|')).toContain(t('vms.hostDevices.needs.noGroup'))
    expect(page()).toContain(t('vms.hostDevices.kinds.usb'))
    expect(page()).toContain('8086:1533')
    expect(page()).toContain(t('vms.hostDevices.driver', { driver: 'igb' }))
    // Two sticks of one kind cannot be told apart.
    expect(page()).toContain(t('vms.hostDevices.twins'))
    expect(document.querySelectorAll('[data-usb]')).toHaveLength(1)
    document.querySelector<HTMLInputElement>('[data-group="6"] input')!.click()
    await settle()
    expect(form.value.pci).toEqual(['0000:06:00.0'])
    document.querySelector<HTMLInputElement>('[data-group="6"] input')!.click()
    await settle()
    expect(form.value.pci).toEqual([])
  })

  it('takes at most 8 USB devices', async () => {
    form.value = {
      ...form.value,
      usb: [
        '1111:0001',
        '1111:0002',
        '1111:0003',
        '1111:0004',
        '1111:0005',
        '1111:0006',
        '1111:0007',
        '1111:0008',
      ],
    }
    await mountApp(Host)
    expect(document.querySelector<HTMLInputElement>('[data-usb="0781:5583"] input')!.disabled).toBe(
      true,
    )
  })

  it('names no USB device where none is plugged in, nor a PCI device where none can go', async () => {
    on('GET', '/api/v1/vms/devices', {
      iommu: { on: true, cmdline: [], cpu_vendor: 'intel' },
      usb: [],
      pci: [],
    })
    await mountApp(Host)
    expect(page()).toContain(t('vms.hostDevices.usbNone'))
    expect(page()).toContain(t('vms.hostDevices.pciNone'))
  })

  it('shows the IOMMU’s command line where it is off', async () => {
    on('GET', '/api/v1/vms/devices', {
      iommu: { on: false, cmdline: ['intel_iommu=off'], cpu_vendor: 'intel' },
      usb: [],
      pci: [],
    })
    await mountApp(Host)
    expect(page()).toContain(t('vms.hostDevices.iommuHint.intel'))
    expect(page()).toContain(t('vms.hostDevices.cmdline', { flags: 'intel_iommu=off' }))
  })
})
