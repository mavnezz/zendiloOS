import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { SmartReport } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import DiskDetail from './DiskDetail.vue'

const report = ref<SmartReport | undefined>()
const startSelfTest = vi.fn<(disk: string, kind: string) => Promise<void>>(() => Promise.resolve())
const abortSelfTest = vi.fn<(disk: string) => Promise<void>>(() => Promise.resolve())
const refresh = vi.fn(() => Promise.resolve())

vi.mock('../api', () => ({
  useDisks: () => ({ data: ref([]) }),
  useSmartReport: () => ({ data: report, error: ref(null), isPending: ref(false) }),
  useRefreshStorage: () => refresh,
  startSelfTest: (disk: string, kind: string) => startSelfTest(disk, kind),
  abortSelfTest: (disk: string) => abortSelfTest(disk),
}))

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
