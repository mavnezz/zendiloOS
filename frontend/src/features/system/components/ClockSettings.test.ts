import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { TimeRequest, TimeState } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import ClockSettings from './ClockSettings.vue'

const time = ref<TimeState>()
const mutate =
  vi.fn<(body: TimeRequest, options: { onSuccess: (state: TimeState) => void }) => void>()

vi.mock('../api', async (original) => ({
  ...(await original<typeof import('../api')>()),
  useTime: () => ({ data: time, isPending: ref(false) }),
  useSetTime: () => ({ mutate, reset: vi.fn(), isPending: ref(false), error: ref(null) }),
}))

// 2026-11-06 09:00:00 UTC.
const NOW = 1_793_955_600

function state(extra: Partial<TimeState> = {}): TimeState {
  return {
    zone: 'Europe/Berlin',
    network_time: true,
    can_network_time: true,
    synchronized: true,
    servers: [],
    server_in_use: '2.debian.pool.ntp.org',
    zones: ['Etc/UTC', 'Europe/Berlin'],
    now: NOW,
    ...extra,
  }
}

function button(wrapper: ReturnType<typeof mount>, text: string) {
  return wrapper.findAll('button').find((b) => b.text() === text)!
}

beforeEach(() => {
  i18n.global.locale.value = 'en'
  time.value = state()
  mutate.mockClear()
})

describe('the clock settings', () => {
  it('show the zone, the NAS’s time in it and the server in use, with nothing to apply', () => {
    const wrapper = mount(ClockSettings, {
      props: { canManage: true },
      global: { plugins: [i18n] },
    })

    expect((wrapper.find('#clock-zone').element as HTMLSelectElement).value).toBe('Europe/Berlin')
    expect(wrapper.text()).toContain('10:00')
    expect(wrapper.text()).toContain('Synchronized with 2.debian.pool.ntp.org')
    expect(wrapper.find('footer').exists()).toBe(false)
  })

  it('apply servers tidied, blank rows left out', async () => {
    const wrapper = mount(ClockSettings, {
      props: { canManage: true },
      global: { plugins: [i18n] },
    })

    await button(wrapper, 'Add server').trigger('click')
    await button(wrapper, 'Add server').trigger('click')
    await wrapper.find('input[aria-label="Time servers 1"]').setValue(' PTB.de ')
    await button(wrapper, 'Apply').trigger('click')

    expect(mutate.mock.calls[0]![0]).toEqual({
      zone: 'Europe/Berlin',
      network_time: true,
      servers: ['PTB.de'],
      clock: undefined,
    })
  })

  it('set the clock by hand in the NAS’s zone, not the browser’s', async () => {
    const wrapper = mount(ClockSettings, {
      props: { canManage: true },
      global: { plugins: [i18n] },
    })

    await wrapper.findAll('[role="radio"]')[1]!.trigger('click')
    await flushPromises()
    await wrapper.find('#clock-time').setValue('2026-11-06T12:30:00')
    await button(wrapper, 'Apply').trigger('click')

    // 12:30 in Berlin, on winter time, is 11:30 UTC.
    expect(mutate.mock.calls[0]![0]).toMatchObject({
      network_time: false,
      clock: NOW + 2.5 * 3600,
    })
  })

  it('are shown but not changed by an account that may not', () => {
    const wrapper = mount(ClockSettings, {
      props: { canManage: false },
      global: { plugins: [i18n] },
    })

    expect(wrapper.find('#clock-zone').attributes('disabled')).toBeDefined()
    expect(button(wrapper, 'Add server').attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('Only administrators change the clock.')
  })
})
