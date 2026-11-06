import { mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { StorageOverview } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import OverviewSection from './OverviewSection.vue'

const overview = ref<StorageOverview | undefined>()

vi.mock('../api', () => ({
  useStorageOverview: () => ({ data: overview, error: ref(null) }),
  usePools: () => ({ data: ref([]) }),
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

describe('the storage overview', () => {
  it('says so when all is well', () => {
    overview.value = base()

    const text = mount(OverviewSection, { global: { plugins: [i18n] } }).text()

    expect(text).toContain('Everything is fine')
    expect(text).toContain('Data disks')
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

    const items = mount(OverviewSection, { global: { plugins: [i18n] } }).findAll('li')

    expect(items.map((item) => item.text())).toEqual([
      'Disk sda (?) is failing. Replace it.',
      'Pool tank is degraded: it works, but with less protection against a disk failing.',
      'from_a_newer_server',
    ])
  })
})
