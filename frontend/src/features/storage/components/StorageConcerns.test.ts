import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { StorageOverview } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import StorageConcerns from './StorageConcerns.vue'

const overview = ref<StorageOverview | undefined>()

vi.mock('../api', () => ({
  useStorageOverview: () => ({ data: overview, error: ref(null) }),
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

beforeEach(() => {
  i18n.global.locale.value = 'en'
})

describe('the storage concerns', () => {
  it('show nothing at all when all is well, or before the answer', () => {
    overview.value = undefined
    expect(mount(StorageConcerns, { global: { plugins: [i18n] } }).html()).toBe('<!--v-if-->')

    overview.value = base()
    expect(mount(StorageConcerns, { global: { plugins: [i18n] } }).html()).toBe('<!--v-if-->')
  })

  it('head the list with the verdict and word each concern, the worst first as sent', () => {
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

    const wrapper = mount(StorageConcerns, { global: { plugins: [i18n] } })

    expect(wrapper.get('[role="status"] h3').text()).toBe('Act now')
    expect(wrapper.findAll('li').map((item) => item.text())).toEqual([
      'Disk sda is failing. Replace it.',
      'Pool tank is degraded: it works, but with less protection against a disk failing.',
      'from_a_newer_server',
    ])
  })

  it('name a disk as the pages do, with its slot and model, and a device as ZFS does otherwise', () => {
    overview.value = {
      ...base(),
      verdict: 'warning',
      concerns: [
        {
          severity: 'warning',
          code: 'device_errors',
          params: {
            pool: 'test',
            device: 'wwn-0x5002538d4130f2e7-part1',
            errors: '3',
            disk: 'sda',
            model: 'SAMSUNG MZ7LN256HCHP',
            serial: 'S1ZENX0H802190',
            slot: 'Bay 1',
            group: 'Front bays',
          },
        },
        {
          severity: 'warning',
          code: 'device_errors',
          params: { pool: 'test', device: 'ata-X-part1', errors: '1', disk: 'sdb', serial: 'S-B' },
        },
        {
          severity: 'warning',
          code: 'device_errors',
          params: { pool: 'test', device: 'wwn-0x5000-part1', errors: '2' },
        },
        {
          severity: 'warning',
          code: 'disk_worn',
          params: { disk: 'sdc', model: 'WD RED', slot: 'Bay 3', group: 'Front bays' },
        },
      ],
    }

    const wrapper = mount(StorageConcerns, { global: { plugins: [i18n] } })

    const items = wrapper.findAll('li p')
    expect(items.map((item) => item.text())).toEqual([
      'sda (Bay 1, Front bays · SAMSUNG MZ7LN256HCHP) in pool test reported 3 read, write or checksum errors.',
      'sdb (S-B) in pool test reported 1 read, write or checksum errors.',
      'wwn-0x5000-part1 in pool test reported 2 read, write or checksum errors.',
      'Disk sdc (Bay 3, Front bays · WD RED) shows signs of wear. Keep a backup and watch it.',
    ])
    expect(items[0]!.attributes('title')).toBe('wwn-0x5002538d4130f2e7-part1')
    expect(items[2]!.attributes('title')).toBeUndefined()
  })

  it('take the tone of the verdict', () => {
    overview.value = {
      ...base(),
      verdict: 'notice',
      concerns: [
        { severity: 'notice', code: 'scrub_overdue', params: { pool: 'tank', days: '41' } },
      ],
    }

    const wrapper = mount(StorageConcerns, { global: { plugins: [i18n] } })

    expect(wrapper.get('h3').text()).toBe('Worth knowing')
    expect(wrapper.get('section').classes()).toContain('border-info')
    expect(wrapper.get('li svg').classes()).toContain('text-info')
    expect(wrapper.text()).toContain('The data on pool tank was last checked 41 days ago.')
  })
})
