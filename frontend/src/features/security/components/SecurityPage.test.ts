import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { BlockedAddress, SecuritySettings, WebTls } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import SecurityPage from './SecurityPage.vue'

const settings = ref<SecuritySettings>()
const tls = ref<WebTls>()
const blocked = ref<BlockedAddress[]>([])
const saveSettings = vi.fn((body: SecuritySettings) => Promise.resolve(body))
const saveTls = vi.fn((body: WebTls) => Promise.resolve(body))
const block = vi.fn()
const mutation = (mutateAsync: unknown, mutate: unknown = vi.fn()) => ({
  mutate,
  mutateAsync,
  reset: vi.fn(),
  isPending: ref(false),
  error: ref(null),
})

vi.mock('../api', () => ({
  useSecuritySettings: () => ({ data: settings, isPending: ref(false) }),
  useTls: () => ({ data: tls }),
  useBlocked: () => ({ data: blocked }),
  useSetSecuritySettings: () =>
    mutation(saveSettings, (body: SecuritySettings) => saveSettings(body)),
  useSetTls: () => mutation(saveTls),
  useBlock: () => mutation(vi.fn(), block),
  useUnblock: () => mutation(vi.fn()),
  useSessions: () => ({ data: ref([]) }),
  useEndSession: () => mutation(vi.fn()),
  useLogins: () => ({ data: ref({ entries: [] }) }),
}))

function page() {
  return mount(SecurityPage, { global: { plugins: [i18n] }, attachTo: document.body })
}

const button = (wrapper: ReturnType<typeof page>, text: string) =>
  wrapper.findAll('button').find((b) => b.text().startsWith(text))!

beforeEach(() => {
  setActivePinia(createPinia())
  document.body.innerHTML = ''
  i18n.global.locale.value = 'en'
  settings.value = {
    idle_minutes: 30,
    account_lock: { failures: 5, window_minutes: 15 },
    auto_block: { failures: 10, window_minutes: 5, release_after_days: 1 },
    allowed: ['192.168.1.0/24'],
  }
  tls.value = { minimum: 'tls1_2' }
  blocked.value = [
    { address: '203.0.113.9', blocked_at: 1_800_000_000, until: null, automatic: true },
  ]
  saveSettings.mockClear()
  saveTls.mockClear()
  block.mockClear()
})

describe('the security page', () => {
  it('shows the settings with nothing to apply, and how many addresses are blocked', () => {
    const wrapper = page()

    expect((wrapper.find('#security-idle').element as HTMLInputElement).value).toBe('30')
    expect((wrapper.find('#security-tls').element as HTMLSelectElement).value).toBe('tls1_2')
    expect(button(wrapper, 'Manage blocked addresses').text()).toContain('1')
    expect(wrapper.find('footer').exists()).toBe(false)
    wrapper.unmount()
  })

  it('applies the settings and the TLS version, keeping the allowed addresses', async () => {
    const wrapper = page()

    await wrapper.find('#security-idle').setValue('60')
    await wrapper.find('#security-release').trigger('click')
    await wrapper.find('#security-tls').setValue('tls1_3')
    await button(wrapper, 'Apply').trigger('click')
    await flushPromises()

    expect(saveSettings).toHaveBeenCalledWith({
      idle_minutes: 60,
      account_lock: { failures: 5, window_minutes: 15 },
      auto_block: { failures: 10, window_minutes: 5, release_after_days: null },
      allowed: ['192.168.1.0/24'],
    })
    expect(saveTls).toHaveBeenCalledWith({ minimum: 'tls1_3' })
    wrapper.unmount()
  })

  it('refuses numbers the server would refuse', async () => {
    const wrapper = page()
    await button(wrapper, 'Account security').trigger('click')

    const failures = wrapper
      .findAll('input')
      .find((i) => (i.element as HTMLInputElement).value === '5')!
    await failures.setValue('2')

    expect(button(wrapper, 'Apply').attributes('disabled')).toBeDefined()
    wrapper.unmount()
  })

  it('only sends what changed: a TLS version alone leaves the settings', async () => {
    const wrapper = page()

    await wrapper.find('#security-tls').setValue('tls1_3')
    await button(wrapper, 'Apply').trigger('click')
    await flushPromises()

    expect(saveSettings).not.toHaveBeenCalled()
    expect(saveTls).toHaveBeenCalledOnce()
    wrapper.unmount()
  })

  it('blocks an address by hand and saves the allowed ones', async () => {
    const wrapper = page()
    await button(wrapper, 'Manage blocked addresses').trigger('click')
    await flushPromises()

    const input = document.querySelector<HTMLInputElement>('[placeholder^="203.0.113.7"]')!
    input.value = '198.51.100.0/24'
    input.dispatchEvent(new Event('input'))
    await flushPromises()
    input.form!.dispatchEvent(new Event('submit'))
    await flushPromises()
    expect(block.mock.calls[0]?.[0]).toBe('198.51.100.0/24')

    const allowedTab = [...document.querySelectorAll('[role="tab"]')].find(
      (tab) => tab.textContent?.trim() === 'Never block',
    ) as HTMLElement
    allowedTab.click()
    await flushPromises()
    const network = document.querySelector<HTMLInputElement>('[placeholder="192.168.1.0/24"]')!
    network.value = '10.8.0.0/16'
    network.dispatchEvent(new Event('input'))
    await flushPromises()
    network.form!.dispatchEvent(new Event('submit'))
    await flushPromises()
    const save = [...document.querySelectorAll('button')].find(
      (b) => b.textContent?.trim() === 'Save',
    )!
    save.click()
    await flushPromises()

    expect(saveSettings.mock.calls[0]?.[0].allowed).toEqual(['192.168.1.0/24', '10.8.0.0/16'])
    wrapper.unmount()
  })
})
