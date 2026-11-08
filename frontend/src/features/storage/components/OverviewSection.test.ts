import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { Enclosure, StorageOverview } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import OverviewSection from './OverviewSection.vue'

const overview = ref<StorageOverview | undefined>()
const enclosure = ref<Enclosure | undefined>()

vi.mock('../api', () => ({
  useStorageOverview: () => ({ data: overview, error: ref(null) }),
  usePools: () => ({ data: ref([]) }),
  useEnclosure: () => ({ data: enclosure }),
  useSmartReport: () => ({ data: ref(undefined) }),
}))

function base(): StorageOverview {
  return {
    verdict: null,
    concerns: [],
    pools: 1,
    disks: 2,
    used_bytes: 1024,
    available_bytes: 3072,
  }
}

function bays(): Enclosure {
  return {
    machine: { vendor: 'UGREEN', product: 'DXP4800 Plus' },
    source: 'model',
    chassis: {
      style: 'tower',
      panel: { side: 'left', power_button: true, leds: ['power', 'network'] },
    },
    groups: [
      {
        id: 'bays',
        name: 'Front bays',
        rows: 1,
        columns: 2,
        orientation: 'vertical',
        form: 'inch_3_5',
        numbering: 'left_to_right',
        slots: [
          {
            id: 'bay1',
            label: 'Bay 1',
            row: 0,
            column: 0,
            overridden: false,
            state: 'occupied',
            disk: 'sda',
            health: 'failing',
          },
          { id: 'bay2', label: 'Bay 2', row: 0, column: 1, overridden: false, state: 'empty' },
        ],
      },
    ],
    unplaced: [],
  }
}

beforeEach(() => {
  i18n.global.locale.value = 'en'
  enclosure.value = undefined
})

describe('the storage overview', () => {
  it('says so when all is well', () => {
    overview.value = base()

    const text = mount(OverviewSection, { global: { plugins: [i18n] } }).text()

    expect(text).toContain('Everything is fine')
    expect(text).toContain('Data disks')
    expect(text).not.toContain('Enclosure')
  })

  it('words each concern, states included, the worst first as sent', () => {
    overview.value = {
      ...base(),
      verdict: 'critical',
      concerns: [
        { severity: 'critical', code: 'disk_failing', params: { disk: 'sda', model: '' } },
        {
          severity: 'warning',
          code: 'pool_degraded',
          params: { pool: 'tank', state: 'degraded' },
        },
        { severity: 'notice', code: 'from_a_newer_server', params: {} },
      ],
    }

    const items = mount(OverviewSection, { global: { plugins: [i18n] } })
      .findAll('li')
      .filter((item) => item.text())

    expect(items.map((item) => item.text())).toEqual([
      'Disk sda (?) is failing. Replace it.',
      'Pool tank is degraded: it works, but with less protection against a disk failing.',
      'from_a_newer_server',
    ])
  })

  it('draws the enclosure in health colours, and a slot opens its disk', async () => {
    overview.value = base()
    enclosure.value = bays()

    const wrapper = mount(OverviewSection, { global: { plugins: [i18n] } })

    expect(wrapper.text()).toContain('Enclosure')
    const tile = wrapper.get('button[data-tone="failing"]')
    expect(tile.attributes('aria-label')).toBe('Bay 1: sda, failing')
    expect(wrapper.find('[data-tone="empty"]').attributes('title')).toBe('Bay 2: –, empty')
    expect(wrapper.text()).not.toContain('Bay 1')
    await tile.trigger('click')
    expect(wrapper.emitted('select')).toEqual([['sda']])
  })
})
