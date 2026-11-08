import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type {
  Disk,
  Enclosure,
  InventoryDisk,
  InventoryUpdate,
  Locating,
  SmartReport,
} from '@/shared/api/types'
import { ApiProblem } from '@/shared/api/client'
import { i18n } from '@/shared/i18n'

import DiskDetail from './DiskDetail.vue'

const report = ref<SmartReport | undefined>()
const disks = ref<Disk[]>([])
const enclosure = ref<Enclosure | undefined>()
const locating = ref<Locating[]>([])
const inventory = ref<InventoryDisk[]>([])
const startSelfTest = vi.fn<(disk: string, kind: string) => Promise<void>>(() => Promise.resolve())
const abortSelfTest = vi.fn<(disk: string) => Promise<void>>(() => Promise.resolve())
const startLocate = vi.fn<(disk: string) => Promise<void>>(() => Promise.resolve())
const stopLocate = vi.fn<(disk: string) => Promise<void>>(() => Promise.resolve())
const saveInventory = vi.fn<(serial: string, body: InventoryUpdate) => Promise<void>>(() =>
  Promise.resolve(),
)
const refetchLocating = vi.fn(() => Promise.resolve())
const refresh = vi.fn(() => Promise.resolve())

vi.mock('../api', () => ({
  useDisks: () => ({ data: disks }),
  useSmartReport: () => ({ data: report, error: ref(null), isPending: ref(false) }),
  useRefreshStorage: () => refresh,
  useEnclosure: () => ({ data: enclosure }),
  useLocating: () => ({ data: locating, refetch: refetchLocating }),
  useInventory: () => ({ data: inventory }),
  startSelfTest: (disk: string, kind: string) => startSelfTest(disk, kind),
  abortSelfTest: (disk: string) => abortSelfTest(disk),
  startLocate: (disk: string) => startLocate(disk),
  stopLocate: (disk: string) => stopLocate(disk),
  saveInventory: (serial: string, body: InventoryUpdate) => saveInventory(serial, body),
}))

function sda(extra: Partial<Disk> = {}): Disk {
  return {
    name: 'sda',
    model: 'WD Red',
    serial: 'WD-1',
    size_bytes: 4e12,
    transport: 'sata',
    media: 'rotational',
    usage: 'unused',
    removable: false,
    read_only: false,
    partitions: [],
    id_path: 'pci-0000:05:00.0-ata-1.0',
    ...extra,
  }
}

function inBay3(): Enclosure {
  return {
    machine: { vendor: 'UGREEN', product: 'DXP4800 Plus' },
    source: 'model',
    chassis: { style: 'tower', panel: null },
    groups: [
      {
        id: 'bays',
        name: 'Front bays',
        rows: 1,
        columns: 1,
        orientation: 'vertical',
        form: 'inch_3_5',
        numbering: 'left_to_right',
        slots: [
          {
            id: 'bay3',
            label: 'Bay 3',
            row: 0,
            column: 0,
            overridden: false,
            state: 'occupied',
            disk: 'sda',
          },
        ],
      },
    ],
    unplaced: [],
  }
}

function available(extra: Partial<SmartReport> = {}): SmartReport {
  return {
    status: 'available',
    attributes: [],
    findings: [],
    short_test_minutes: 2,
    long_test_minutes: 125,
    self_tests_supported: true,
    ...extra,
  }
}

function mountDetail(canManage = true) {
  return mount(DiskDetail, {
    props: { name: 'sda', canManage },
    global: { plugins: [i18n] },
  })
}

function button(wrapper: ReturnType<typeof mountDetail>, text: string) {
  const found = wrapper.findAll('button').find((b) => b.text().includes(text))
  if (!found) throw new Error(`no button "${text}"`)
  return found
}

beforeEach(() => {
  i18n.global.locale.value = 'en'
  startSelfTest.mockClear()
  abortSelfTest.mockClear()
  startLocate.mockClear()
  stopLocate.mockClear()
  saveInventory.mockClear()
  disks.value = []
  enclosure.value = undefined
  locating.value = []
  inventory.value = []
})

describe('where a disk sits', () => {
  it('names its slot and has it blink, with a countdown, until stopped', async () => {
    vi.useFakeTimers({ now: 1_800_000_000_000 })
    try {
      disks.value = [sda()]
      enclosure.value = inBay3()
      const wrapper = mountDetail()
      expect(wrapper.text()).toContain('Bay 3, Front bays')
      expect(wrapper.text()).toContain('Nothing is written.')

      await button(wrapper, 'Locate').trigger('click')
      await flushPromises()
      expect(startLocate).toHaveBeenCalledWith('sda')
      expect(refresh).toHaveBeenCalled()

      locating.value = [{ disk: 'sda', until: 1_800_000_060 }]
      await flushPromises()
      expect(wrapper.text()).toContain('1:00 left')
      vi.advanceTimersByTime(15_000)
      await flushPromises()
      expect(wrapper.text()).toContain('0:45 left')
      vi.advanceTimersByTime(46_000)
      expect(refetchLocating).toHaveBeenCalled()

      await button(wrapper, 'Stop').trigger('click')
      await flushPromises()
      expect(stopLocate).toHaveBeenCalledWith('sda')
      wrapper.unmount()
    } finally {
      vi.useRealTimers()
    }
  })

  it('says an M.2 slot mostly has no light, and offers nothing to viewers', () => {
    disks.value = [sda({ name: 'sda', transport: 'nvme' })]
    const wrapper = mountDetail(false)
    expect(wrapper.text()).toContain('M.2 slots mostly have no light')
    expect(wrapper.findAll('button').some((b) => b.text().includes('Locate'))).toBe(false)
    expect(wrapper.text()).not.toContain('Front bays')
  })

  it('says why it cannot blink', async () => {
    disks.value = [sda()]
    startLocate.mockRejectedValueOnce(new ApiProblem(404, 'not_found'))
    const wrapper = mountDetail()
    await button(wrapper, 'Locate').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain(i18n.global.t('errors.not_found'))
  })
})

describe('a disk’s purchase and warranty', () => {
  function seen(extra: Partial<InventoryDisk> = {}): InventoryDisk {
    return {
      serial: 'WD-1',
      size_bytes: 4e12,
      first_seen: 1_790_000_000,
      last_seen: 1_800_000_000,
      disk: 'sda',
      ...extra,
    }
  }

  it('are noted for the disk itself where another one reports its serial number', async () => {
    const dock = 'pci-0000:00:14.0-usb-0:2:1.0-scsi-0:0:0'
    disks.value = [sda({ serial: 'BRIDGE1', transport: 'usb', id_path: `${dock}:0` })]
    inventory.value = [
      seen({ serial: `BRIDGE1@${dock}:1`, disk: 'sdb', vendor: 'Other shop' }),
      seen({ serial: `BRIDGE1@${dock}:0`, vendor: 'Shop' }),
    ]
    const wrapper = mountDetail()
    expect((wrapper.get('#inventory-vendor').element as HTMLInputElement).value).toBe('Shop')

    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(saveInventory).toHaveBeenCalledWith(`BRIDGE1@${dock}:0`, expect.anything())
  })

  it('are noted by serial number', async () => {
    disks.value = [sda()]
    inventory.value = [seen({ vendor: 'Shop', purchased: '2024-03-01' })]
    const wrapper = mountDetail()
    expect((wrapper.get('#inventory-vendor').element as HTMLInputElement).value).toBe('Shop')
    expect(wrapper.text()).toContain('First seen')

    await wrapper.get('#inventory-warranty').setValue('2027-03-01')
    await wrapper.get('#inventory-note').setValue('RMA once')
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(saveInventory).toHaveBeenCalledWith('WD-1', {
      purchased: '2024-03-01',
      warranty_until: '2027-03-01',
      vendor: 'Shop',
      note: 'RMA once',
    })
    expect(wrapper.text()).toContain('Saved.')
  })

  it('say why they were not kept, and need a serial number', async () => {
    disks.value = [sda()]
    inventory.value = [seen()]
    saveInventory.mockRejectedValueOnce(
      new ApiProblem(422, 'invalid_input', 'the warranty cannot end before the disk was bought'),
    )
    const wrapper = mountDetail()
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(wrapper.text()).toContain('the warranty cannot end before')

    disks.value = [sda({ serial: null })]
    await flushPromises()
    expect(wrapper.text()).toContain('reports no serial number')
  })

  it('are only shown to viewers', () => {
    disks.value = [sda()]
    inventory.value = [seen()]
    const wrapper = mountDetail(false)
    expect(wrapper.get('fieldset').attributes('disabled')).toBeDefined()
    expect(wrapper.findAll('button').some((b) => b.text() === 'Save')).toBe(false)
  })
})

describe('a disk’s self-test', () => {
  it('can be started short or long, the long one saying how long it takes', async () => {
    report.value = available()
    const wrapper = mountDetail()

    expect(button(wrapper, 'Long test').text()).toContain('2 h 5 min')
    await button(wrapper, 'Short test').trigger('click')
    await flushPromises()

    expect(startSelfTest).toHaveBeenCalledWith('sda', 'short')
    expect(refresh).toHaveBeenCalled()
  })

  it('shows how far a running test is and can stop it', async () => {
    report.value = available({ running_self_test: { remaining_percent: 70 } })
    const wrapper = mountDetail()

    expect(wrapper.text()).toContain('30 % done')
    expect(wrapper.text()).not.toContain('Short test')
    await button(wrapper, 'Stop the test').trigger('click')
    await flushPromises()

    expect(abortSelfTest).toHaveBeenCalledWith('sda')
  })

  it('says so when the disk cannot test itself', () => {
    report.value = available({ self_tests_supported: false })

    const text = mountDetail().text()

    expect(text).toContain('This disk cannot test itself.')
    expect(text).not.toContain('Short test')
  })

  it('offers nothing to start to those who may only look', () => {
    report.value = available()

    const text = mountDetail(false).text()

    expect(text).toContain('Self-test')
    expect(text).not.toContain('Short test')
  })
})
