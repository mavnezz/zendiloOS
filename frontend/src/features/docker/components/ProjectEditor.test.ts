import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { ComposeRisk, NewDockerProject } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import ProjectEditor from './ProjectEditor.vue'

let risks: ComposeRisk[] = []
const check = vi.fn<(body: unknown) => Promise<{ risks: ComposeRisk[] }>>(() =>
  Promise.resolve({ risks }),
)
const create = vi.fn((_body: NewDockerProject, options: { onSuccess: () => void }) =>
  options.onSuccess(),
)

vi.mock('../api', () => ({
  usePlaces: () => ({ data: ref([{ share_id: 3, share: 'docker' }]) }),
  checkCompose: (body: unknown) => check(body),
  useCreateProject: () => ({ mutate: create, isPending: ref(false) }),
  useChangeCompose: () => ({ mutate: vi.fn(), isPending: ref(false) }),
}))

function editor() {
  return mount(ProjectEditor, { global: { plugins: [i18n] }, attachTo: document.body })
}

async function fill(wrapper: ReturnType<typeof editor>): Promise<void> {
  await wrapper.find('#project-name').setValue('tesla')
  await wrapper.find('textarea').setValue('services:\n  app:\n    image: nginx\n')
  await wrapper
    .findAll('button')
    .find((b) => b.text() === 'Deploy')!
    .trigger('click')
  await flushPromises()
}

beforeEach(() => {
  setActivePinia(createPinia())
  document.body.innerHTML = ''
  i18n.global.locale.value = 'en'
  risks = []
  check.mockClear()
  create.mockClear()
})

describe('the project editor', () => {
  it('makes a plain project in the folder named after it', async () => {
    const wrapper = editor()

    await fill(wrapper)

    expect(check).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'tesla', share_id: 3, folder: 'docker/tesla' }),
    )
    expect(create.mock.calls[0]?.[0]).toMatchObject({
      name: 'tesla',
      start: true,
      accept_risks: false,
    })
    expect(wrapper.emitted('done')).toEqual([['tesla']])
    wrapper.unmount()
  })

  it('asks before it runs what reaches beyond its containers', async () => {
    risks = [{ service: 'app', kind: 'docker_socket', detail: '/var/run/docker.sock' }]
    const wrapper = editor()

    await fill(wrapper)

    expect(create).not.toHaveBeenCalled()
    expect(document.body.textContent).toContain('access to Docker itself')
    expect(document.body.textContent).toContain('/var/run/docker.sock')
    const confirm = [...document.querySelectorAll('button')].find(
      (b) => b.textContent?.trim() === 'Deploy anyway',
    )!
    confirm.click()
    await flushPromises()
    expect(create.mock.calls[0]?.[0]).toMatchObject({ accept_risks: true })
    wrapper.unmount()
  })

  it('refuses a name Compose would not take', async () => {
    const wrapper = editor()
    await wrapper.find('#project-name').setValue('Tesla')
    await wrapper.find('textarea').setValue('services: {}')

    const deploy = wrapper.findAll('button').find((b) => b.text() === 'Deploy')!
    expect(deploy.attributes('disabled')).toBeDefined()
    wrapper.unmount()
  })
})
