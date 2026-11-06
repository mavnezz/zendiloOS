import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { SshRequest, SshState } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import TerminalPage from './TerminalPage.vue'

const ssh = ref<SshState>()
const mutate =
  vi.fn<(body: SshRequest, options: { onSuccess: (state: SshState) => void }) => void>()

vi.mock('../api', async (original) => ({
  ...(await original<typeof import('../api')>()),
  useSsh: () => ({ data: ssh, isPending: ref(false) }),
  useSetSsh: () => ({ mutate, reset: vi.fn(), isPending: ref(false), error: ref(null) }),
}))

function state(extra: Partial<SshState> = {}): SshState {
  return {
    installed: true,
    enabled: false,
    running: false,
    port: 22,
    root_login: null,
    password_login: null,
    off_at: null,
    ...extra,
  }
}

function page() {
  return mount(TerminalPage, {
    props: { hostname: 'nas', address: '192.168.1.90' },
    global: { plugins: [i18n] },
    attachTo: document.body,
  })
}

function button(wrapper: ReturnType<typeof mount>, text: string) {
  return wrapper.findAll('button').find((b) => b.text() === text)!
}

beforeEach(() => {
  setActivePinia(createPinia())
  document.body.innerHTML = ''
  i18n.global.locale.value = 'en'
  ssh.value = state()
  mutate.mockClear()
})

describe('the terminal page', () => {
  it('shows SSH off with nothing to apply, and how to connect', () => {
    const wrapper = page()

    expect(wrapper.text()).toContain('Off')
    expect(wrapper.text()).toContain('ssh user@nas.local')
    expect(wrapper.text()).toContain('ssh user@192.168.1.90')
    expect(wrapper.find('footer').exists()).toBe(false)
  })

  it('turns SSH on at another port, off again after half an hour', async () => {
    const wrapper = page()

    await wrapper.find('#ssh-enabled').trigger('click')
    await wrapper.find('#ssh-port').setValue('2222')
    await wrapper.find('#ssh-off').setValue('30')
    await button(wrapper, 'Apply').trigger('click')

    expect(mutate.mock.calls[0]?.[0]).toEqual({
      enabled: true,
      port: 2222,
      root_login: null,
      password_login: null,
      off_after_minutes: 30,
    })
  })

  it('refuses a port a service of the NAS uses', async () => {
    ssh.value = state({ enabled: true, running: true })
    const wrapper = page()

    await wrapper.find('#ssh-port').setValue('445')

    expect(wrapper.text()).toContain('Only 22 or a port from 1024')
    expect(button(wrapper, 'Apply').attributes('disabled')).toBeDefined()
  })

  it('names the planned time to turn off and keeps it', () => {
    const offAt = Math.floor(Date.now() / 1000) + 600
    ssh.value = state({ enabled: true, running: true, port: 2222, off_at: offAt })
    const wrapper = page()

    expect(wrapper.text()).toContain('Turns itself off at')
    expect(wrapper.text()).toContain('ssh -p 2222 user@nas.local')
    expect((wrapper.find('#ssh-off').element as HTMLSelectElement).value).toBe('planned')
  })

  it('sends the rules chosen under Advanced', async () => {
    ssh.value = state({ enabled: true, running: true })
    const wrapper = page()

    await button(wrapper, 'Advanced').trigger('click')
    await flushPromises()
    document.querySelector<HTMLButtonElement>('[role="radio"][value="key_only"]')!.click()
    await flushPromises()
    const save = [...document.querySelectorAll('button')].find(
      (b) => b.textContent?.trim() === 'Save',
    )!
    save.click()
    await flushPromises()

    expect(mutate.mock.calls[0]?.[0]).toMatchObject({
      enabled: true,
      root_login: 'key_only',
      password_login: null,
    })
    wrapper.unmount()
  })

  it('says how to install the server when there is none', () => {
    ssh.value = state({ installed: false })
    const wrapper = page()

    expect(wrapper.text()).toContain('apt install openssh-server')
    expect(wrapper.find('#ssh-enabled').attributes('disabled')).toBeDefined()
  })
})
