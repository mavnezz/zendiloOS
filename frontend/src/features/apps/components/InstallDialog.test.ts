import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { CatalogAppDetail, ComposeRisk, FillTemplateRequest } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'

import InstallDialog from './InstallDialog.vue'

const fillTemplate = vi.fn<(app: unknown, values: FillTemplateRequest) => Promise<string>>(() =>
  Promise.resolve('services:\n  immich: {}\n'),
)
const checkCompose = vi.fn<(body: unknown) => Promise<{ risks: ComposeRisk[]; digest: string }>>(
  () => Promise.resolve({ risks: [], digest: 'c0ffee' }),
)
const createProject = vi.fn((body: unknown) =>
  Promise.resolve({
    body,
    id: 9,
    kind: 'containers.project_deploy',
    target: 'photos',
    state: 'running' as const,
    created_at: 0,
    updated_at: 0,
  }),
)
const refresh = vi.fn(() => Promise.resolve())
const listFolders = vi.fn((_share: number, path: string) =>
  Promise.resolve({ names: path === '' ? ['2024', 'Old'] : [], writable: true }),
)

vi.mock('../api', () => ({
  fillTemplate: (app: unknown, values: FillTemplateRequest) => fillTemplate(app, values),
  checkCompose: (body: unknown) => checkCompose(body),
  createProject: (body: unknown) => createProject(body),
  useInstallActions: () => ({ refresh }),
  usePlaces: () => ({
    data: ref([
      { share_id: 4, share: 'docker' },
      { share_id: 1, share: 'family' },
    ]),
  }),
  useShareList: () => ({
    data: ref([
      { id: 1, name: 'family', writable: true },
      { id: 2, name: 'media', writable: false },
    ]),
  }),
  listFolders: (share: number, path: string) => listFolders(share, path),
  makeFolder: () => Promise.resolve(),
}))
// The folder picker asks for the same, through the shared module.
vi.mock('@/shared/ui/folder-picker/folders', () => ({
  useShareList: () => ({
    data: ref([
      { id: 1, name: 'family', writable: true },
      { id: 2, name: 'media', writable: false },
    ]),
  }),
  listFolders: (share: number, path: string) => listFolders(share, path),
  makeFolder: () => Promise.resolve(),
}))

const immich: CatalogAppDetail = {
  source_id: 1,
  official: true,
  id: 'immich',
  name: 'Immich',
  version: '2.0',
  tagline: 'Photos from your phone',
  categories: ['photos'],
  permissions: {
    devices: ['/dev/dri'],
    capabilities: [],
    host_network: false,
    docker_socket: false,
  },
  translations: {},
  digest: 'sha256:immich',
  description: 'Backs up photos.',
  developer: 'Immich people',
  website: 'https://immich.app',
  source_code: 'https://github.com/immich-app/immich',
  license: 'AGPL-3.0',
  architectures: ['amd64'],
  notes: 'Create the first account in the browser.',
  web: { port: 'web', path: '/', https: false },
  ports: [{ id: 'web', label: 'Web interface', default: 2283, fixed: false }],
  shares: [{ id: 'photos', label: 'Photos', description: null, read_write: true }],
  settings: [
    {
      id: 'DB_PASSWORD',
      label: 'Database password',
      description: null,
      kind: 'password',
      default: null,
      required: true,
      generate: true,
    },
    {
      id: 'ADMIN_EMAIL',
      label: 'Administrator email',
      description: null,
      kind: 'text',
      default: null,
      required: true,
      generate: false,
    },
  ],
  compose: 'services: {}',
}

function field(id: string): HTMLInputElement | HTMLSelectElement {
  const element = document.body.querySelector<HTMLInputElement | HTMLSelectElement>(`#${id}`)
  if (!element) throw new Error(`no #${id}`)
  return element
}

async function type(id: string, value: string): Promise<void> {
  const element = field(id)
  element.value = value
  element.dispatchEvent(new Event(element instanceof HTMLSelectElement ? 'change' : 'input'))
  await flushPromises()
}

function button(text: string): HTMLButtonElement {
  const found = [...document.body.querySelectorAll('button')].find(
    (b) => b.textContent?.trim() === text,
  )
  if (!found) throw new Error(`no button ${text}: ${document.body.textContent}`)
  return found
}

async function press(text: string): Promise<void> {
  button(text).click()
  await flushPromises()
}

/** The folders of the picker's top level: the shares offered. */
function sharesOffered(): string[] {
  return [...document.body.querySelectorAll('[role=treeitem]')].map(
    (row) => row.textContent?.trim() ?? '',
  )
}

/** Opens the picker of `id` and picks the folder `path`, opening folder by folder. */
async function pickFolder(id: string, path: string[]): Promise<void> {
  field(id).dispatchEvent(new MouseEvent('click', { bubbles: true }))
  await flushPromises()
  for (const [index, name] of path.entries()) {
    const item = [...document.body.querySelectorAll('[role=treeitem]')].find(
      (row) => row.textContent?.trim() === name,
    )
    if (!item) throw new Error(`no folder ${name}`)
    const [toggle, choose] = [...item.querySelectorAll('button')]
    if (index < path.length - 1) toggle!.click()
    else choose!.click()
    await flushPromises()
  }
  await press('Confirm')
}

async function opened() {
  const pinia = createPinia()
  setActivePinia(pinia)
  const wrapper = mount(InstallDialog, {
    props: { open: false, app: immich },
    global: { plugins: [i18n, pinia] },
    attachTo: document.body,
  })
  await wrapper.setProps({ open: true })
  await flushPromises()
  return wrapper
}

/** Through the first two steps, with a folder of `family` for the photos. */
async function toTheFile(): Promise<void> {
  await press('Next')
  await pickFolder('share-photos', ['family', '2024'])
  await type('setting-ADMIN_EMAIL', 'me@example.org')
  await type('port-web', '2300')
  await press('Next')
}

beforeEach(() => {
  i18n.global.locale.value = 'en'
  for (const mock of [fillTemplate, checkCompose, createProject, refresh, listFolders])
    mock.mockClear()
})

afterEach(() => {
  document.body.innerHTML = ''
})

describe('the install dialog', () => {
  it('makes the app a project in a folder of a share, from its file filled in', async () => {
    const wrapper = await opened()

    expect(document.body.textContent).toContain('Where the project goes')
    expect(field('install-name').value).toBe('immich')
    expect(field('install-folder').value).toBe('docker/immich')
    await type('install-name', 'photos')
    expect(field('install-folder').value).toBe('docker/photos')
    await press('Next')

    expect(document.body.textContent).toContain('Immich · folder: docker/photos')
    expect(document.body.textContent).toContain('zendiloOS makes up: Database password')
    expect(document.body.textContent).toContain('Create the first account in the browser.')
    expect(button('Next').disabled).toBe(true)
    await pickFolder('share-photos', ['family', '2024'])
    expect(listFolders).toHaveBeenCalledWith(1, '')
    await type('setting-ADMIN_EMAIL', 'me@example.org')
    await type('port-web', '2300')
    await press('Next')

    expect(fillTemplate).toHaveBeenCalledWith(immich, {
      ports: { web: 2300 },
      shares: { photos: 'family/2024' },
      settings: { ADMIN_EMAIL: 'me@example.org' },
    })
    expect(document.body.textContent).toContain('The Compose file')
    expect(document.body.textContent).toContain('docker/photos/compose.yaml')
    const editor = document.body.querySelector('textarea')!
    expect(editor.value).toBe('services:\n  immich: {}\n')
    editor.value = 'services:\n  immich: { restart: always }\n'
    editor.dispatchEvent(new Event('input'))
    await press('Install')

    const place = { name: 'photos', share_id: 4, folder: 'photos' }
    const compose = 'services:\n  immich: { restart: always }\n'
    expect(checkCompose).toHaveBeenCalledWith({ ...place, compose })
    expect(createProject).toHaveBeenCalledWith({
      ...place,
      compose,
      accepted: null,
      template: { source_id: 1, app: 'immich' },
    })
    expect(useJobsStore().running.map((job) => job.id)).toEqual([9])
    expect(refresh).toHaveBeenCalled()
    // What Compose does shows as it runs; the dialog closes once it is read.
    expect(document.body.textContent).toContain('Deployment')
    expect(wrapper.emitted('installed')).toBeUndefined()
    useJobsStore().update({
      id: 9,
      kind: 'containers.project_deploy',
      target: 'photos',
      state: 'succeeded',
      created_at: 0,
      updated_at: 1,
      output: ['Container photos-immich-1 Started'],
    })
    await flushPromises()
    expect(document.body.textContent).toContain('Container photos-immich-1 Started')
    await press('OK')
    expect(wrapper.emitted('installed')).toEqual([['photos']])
  })

  it('picks the base folder in the folder browser, a share by itself getting the project’s name', async () => {
    await opened()
    const browse = () => button('Browse…').dispatchEvent(new MouseEvent('click', { bubbles: true }))

    browse()
    await flushPromises()
    expect(sharesOffered()).toEqual(['family'])
    const family = [...document.body.querySelectorAll('[role=treeitem]')][0]!
    family.querySelectorAll('button')[1]!.click()
    await flushPromises()
    await press('Confirm')
    expect(field('install-folder').value).toBe('family/immich')
    await type('install-name', 'photos')
    expect(field('install-folder').value).toBe('family/immich')

    // It opens where the folder picked before is: family's folders are shown.
    browse()
    await flushPromises()
    const old = [...document.body.querySelectorAll('[role=treeitem]')].find(
      (row) => row.textContent?.trim() === 'Old',
    )!
    old.querySelectorAll('button')[1]!.click()
    await flushPromises()
    await press('Confirm')
    expect(field('install-folder').value).toBe('family/Old')
    expect(button('Next').disabled).toBe(false)
  })

  it('says when the base folder is not in a share a project may go in', async () => {
    await opened()
    await type('install-folder', 'media/immich')
    expect(document.body.textContent).toContain('Choose a folder in a share you may write to')
    expect(button('Next').disabled).toBe(true)
    await type('install-folder', 'docker')
    expect(button('Next').disabled).toBe(true)
    await type('install-folder', '/docker/immich/')
    expect(button('Next').disabled).toBe(false)
  })

  it('asks before what reaches beyond the containers runs', async () => {
    await opened()
    checkCompose.mockResolvedValueOnce({
      risks: [{ service: 'immich', kind: 'devices', detail: '/dev/dri' }],
      digest: 'd1g35t',
    })
    await toTheFile()

    await press('Install')
    expect(createProject).not.toHaveBeenCalled()
    expect(document.body.textContent).toContain('Containers with access to the system')
    await press('Deploy anyway')

    // The confirmation holds for what was checked, and nothing else.
    expect(createProject).toHaveBeenCalledWith(expect.objectContaining({ accepted: 'd1g35t' }))
  })

  it('offers only shares the app may write to where it writes, and says what failed', async () => {
    await opened()
    await type('install-name', 'Not A Name')
    expect(button('Next').disabled).toBe(true)
    await type('install-name', 'photos')
    await press('Next')

    field('share-photos').dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await flushPromises()
    expect(sharesOffered()).toEqual(['family'])
    await pickFolder('share-photos', ['family'])
    await type('setting-ADMIN_EMAIL', 'me@example.org')
    fillTemplate.mockRejectedValueOnce(new Error('offline'))
    await press('Next')
    expect(document.body.querySelector('[role=alert]')).not.toBeNull()
    expect(document.body.textContent).not.toContain('The Compose file')

    await press('Back')
    expect(document.body.textContent).toContain('Where the project goes')
  })
})
