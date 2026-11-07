import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { FileServices } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import FileServicesPage from './FileServicesPage.vue'

const services = ref<FileServices>()
const mutate = vi.fn<(body: FileServices) => void>()

vi.mock('../../sharing', async (original) => ({
  ...(await original<typeof import('../../sharing')>()),
  useFileServices: () => ({ data: services, isPending: ref(false) }),
  useSetFileServices: () => ({ mutate, reset: vi.fn(), isPending: ref(false), error: ref(null) }),
}))
vi.mock('@tanstack/vue-query', async (original) => ({
  ...(await original<typeof import('@tanstack/vue-query')>()),
  useQuery: () => ({ data: ref([{ id: 3, name: 'backups', shared: true }]) }),
}))

function page(canManage = true) {
  return mount(FileServicesPage, {
    props: { canManage, hostname: 'nas', address: '192.168.1.9' },
    global: { plugins: [i18n] },
    attachTo: document.body,
  })
}

const button = (wrapper: ReturnType<typeof page>, text: string) =>
  wrapper.findAll('button').find((b) => b.text() === text)

beforeEach(() => {
  setActivePinia(createPinia())
  i18n.global.locale.value = 'en'
  document.body.innerHTML = ''
  services.value = {
    smb: true,
    nfs: true,
    workgroup: 'WORKGROUP',
    smb_min_protocol: 'smb2_1',
    smb_encryption: 'default',
    smb_signing: 'default',
    hide_unreadable: true,
    multichannel: true,
    oplocks: true,
    leases: true,
    durable_handles: false,
    mac_compat: true,
    follow_symlinks: true,
    veto_files: null,
    delete_veto_files: false,
    wsdd: true,
    bonjour: true,
    time_machine: null,
  }
  mutate.mockClear()
})

describe('the file services page', () => {
  it('shows SMB with where to connect, and nothing to apply', () => {
    const wrapper = page()

    expect((wrapper.find('#smb-workgroup').element as HTMLInputElement).value).toBe('WORKGROUP')
    expect(wrapper.text()).toContain('\\\\nas')
    expect(wrapper.text()).toContain('smb://nas.local')
    expect(wrapper.text()).toContain('\\\\192.168.1.9')
    expect(wrapper.find('footer').exists()).toBe(false)
  })

  it('applies the workgroup uppercase', async () => {
    const wrapper = page()

    await wrapper.find('#smb-workgroup').setValue(' home ')
    await button(wrapper, 'Apply')!.trigger('click')

    expect(mutate.mock.calls[0]![0]).toMatchObject({
      workgroup: 'HOME',
      smb: true,
      time_machine: null,
    })
  })

  it('names the protocols still to come', async () => {
    const wrapper = page()

    await wrapper
      .findAll('[role="tab"]')
      .find((tab) => tab.text() === 'rsync')!
      .trigger('click')

    expect(wrapper.text()).toContain('Coming later')
  })

  it('chooses a Time Machine folder with a limit', async () => {
    const wrapper = page()

    await wrapper
      .findAll('[role="tab"]')
      .find((tab) => tab.text() === 'Advanced')!
      .trigger('click')
    await wrapper.find('#time-machine-share').setValue('3')
    await wrapper.find('input[type="number"]').setValue('500')
    await button(wrapper, 'Apply')!.trigger('click')

    expect(mutate.mock.calls[0]![0].time_machine).toEqual({
      share_id: 3,
      max_bytes: 500 * 1024 ** 3,
    })
  })

  it('saves the advanced SMB options from their dialog', async () => {
    const wrapper = page()

    const opener = wrapper
      .findAll('button')
      .find((b) => b.text() === 'Advanced' && b.attributes('role') !== 'tab')!
    await opener.trigger('click')
    await flushPromises()
    const signing = document.querySelector<HTMLSelectElement>('#smb-signing')!
    signing.value = 'mandatory'
    signing.dispatchEvent(new Event('change'))
    await flushPromises()
    const save = [...document.querySelectorAll('button')].find(
      (b) => b.textContent?.trim() === 'Save',
    )!
    save.click()
    await flushPromises()

    expect(mutate.mock.calls[0]![0]).toMatchObject({ smb_signing: 'mandatory', leases: true })
    wrapper.unmount()
  })
})
