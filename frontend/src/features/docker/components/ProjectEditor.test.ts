import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { ComposeRisk, Job, NewDockerProject } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { job } from '@/shared/testing/fixtures'

import ProjectEditor from './ProjectEditor.vue'

let risks: ComposeRisk[] = []
const check = vi.fn<(body: unknown) => Promise<{ risks: ComposeRisk[]; digest: string }>>(() =>
  Promise.resolve({ risks, digest: 'c0ffee' }),
)
/** What making the project answers: the deploying job, if it starts. */
let created: { job: Job | null } = { job: null }
const create = vi.fn(
  (_body: NewDockerProject, options: { onSuccess: (made: { job: Job | null }) => void }) =>
    options.onSuccess(created),
)

const change = vi.fn()

vi.mock('../api', () => ({
  usePlaces: () => ({ data: ref([{ share_id: 3, share: 'docker' }]) }),
  checkCompose: (body: unknown) => check(body),
  useCreateProject: () => ({ mutate: create, isPending: ref(false) }),
  useChangeCompose: () => ({ mutate: change, isPending: ref(false) }),
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
  created = { job: null }
  check.mockClear()
  create.mockClear()
  change.mockClear()
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
      accepted: null,
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
    expect(create.mock.calls[0]?.[0]).toMatchObject({ accepted: 'c0ffee' })
    wrapper.unmount()
  })

  it('shows what Compose writes while it deploys, the project once closed', async () => {
    created = {
      job: job(9, 'containers.project_deploy', {
        target: 'tesla',
        output: [' Container tesla-app-1  Started'],
      }),
    }
    const wrapper = editor()

    await fill(wrapper)

    expect(document.body.textContent).toContain('Container tesla-app-1  Started')
    expect(document.body.textContent).toContain('Deploying')
    expect(wrapper.emitted('done')).toBeUndefined()
    useJobsStore().update(
      job(9, 'containers.project_deploy', {
        state: 'succeeded',
        output: [' Container tesla-app-1  Started'],
      }),
    )
    await flushPromises()
    const ok = [...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === 'OK')!
    ok.click()
    await flushPromises()
    expect(wrapper.emitted('done')).toEqual([['tesla']])
    wrapper.unmount()
  })

  it('saves a changed file and deploys it, with the newest images when asked', async () => {
    const wrapper = mount(ProjectEditor, {
      props: { name: 'tesla', compose: 'services: {}\n' },
      global: { plugins: [i18n] },
      attachTo: document.body,
    })
    const save = () =>
      wrapper
        .findAll('button')
        .find((b) => b.text() === 'Save and redeploy')!
        .trigger('click')
    await save()
    await flushPromises()
    expect(change.mock.calls[0]![0]).toEqual(
      expect.objectContaining({ name: 'tesla', pull: false, accepted: null }),
    )
    await wrapper.find('input[type=checkbox]').setValue(true)
    await save()
    await flushPromises()
    expect(change.mock.calls[1]![0]).toEqual(expect.objectContaining({ pull: true }))
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
