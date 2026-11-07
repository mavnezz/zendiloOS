import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { CatalogAppDetail, InstallAppRequest } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import InstallDialog from './InstallDialog.vue'

const install = vi.fn<(request: InstallAppRequest) => Promise<void>>(() => Promise.resolve())
const listFolders = vi.fn((_share: number, path: string) =>
  Promise.resolve({ names: path === '' ? ['2024', 'Old'] : [], writable: true }),
)

vi.mock('../api', () => ({
  useInstallActions: () => ({ install: (request: InstallAppRequest) => install(request) }),
  useShareList: () => ({
    data: ref([
      { id: 1, name: 'family' },
      { id: 2, name: 'media' },
    ]),
  }),
  listFolders: (share: number, path: string) => listFolders(share, path),
  makeFolder: () => Promise.resolve(),
  useDocker: () => ({
    data: ref({
      installed: true,
      running: true,
      volume: { pool: 'tank', number: 1 },
      volumes: [
        { pool: 'tank', number: 1 },
        { pool: 'backup', number: 2 },
      ],
    }),
  }),
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
  notes: null,
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

function installButton(): HTMLButtonElement {
  const button = [...document.body.querySelectorAll('button')].find(
    (b) => b.textContent?.trim() === 'Install',
  )
  if (!button) throw new Error('no install button')
  return button
}

beforeEach(() => {
  i18n.global.locale.value = 'en'
  install.mockClear()
  window.localStorage.clear()
})

function button(text: string): HTMLButtonElement {
  const found = [...document.body.querySelectorAll('button')].find(
    (b) => b.textContent?.trim() === text,
  )
  if (!found) throw new Error(`no button ${text}`)
  return found
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
  button('Confirm').click()
  await flushPromises()
}

afterEach(() => {
  document.body.innerHTML = ''
})

describe('the install dialog', () => {
  it('asks for shares, settings and the permissions before installing', async () => {
    const wrapper = mount(InstallDialog, {
      props: { open: false, app: immich },
      global: { plugins: [i18n] },
      attachTo: document.body,
    })
    await wrapper.setProps({ open: true })
    await flushPromises()

    expect(document.body.textContent).toContain('Choose a location')
    expect((field('install-pool') as HTMLSelectElement).value).toBe('tank')
    await type('install-pool', 'backup')
    button('Next').click()
    await flushPromises()

    expect(document.body.textContent).toContain('location: Volume 2 (backup)')
    expect(document.body.textContent).toContain('zendiloOS makes up: Database password')
    expect(field('port-web').value).toBe('2283')
    expect(installButton().disabled).toBe(true)

    await pickFolder('share-photos', ['family', '2024'])
    expect(listFolders).toHaveBeenCalledWith(1, '')
    expect(field('share-photos').textContent).toContain('family/2024')
    await type('setting-ADMIN_EMAIL', 'me@example.org')
    await type('port-web', '2300')
    expect(installButton().disabled).toBe(true)
    const confirm = [
      ...document.body.querySelectorAll<HTMLInputElement>('input[type=checkbox]'),
    ].at(-1)!
    confirm.click()
    await flushPromises()
    expect(installButton().disabled).toBe(false)

    document.body.querySelector('form')?.dispatchEvent(new Event('submit'))
    await flushPromises()
    expect(install).toHaveBeenCalledWith({
      source: 1,
      app: 'immich',
      ports: { web: 2300 },
      shares: { photos: 'family/2024' },
      settings: { ADMIN_EMAIL: 'me@example.org' },
      confirm_permissions: true,
      digest: 'sha256:immich',
      pool: 'backup',
    })
    expect(wrapper.emitted('installed')).toEqual([['immich']])
  })

  it('asks no more where apps go once that is to stay', async () => {
    const wrapper = mount(InstallDialog, {
      props: { open: false, app: immich },
      global: { plugins: [i18n] },
      attachTo: document.body,
    })
    await wrapper.setProps({ open: true })
    await flushPromises()
    await type('install-pool', 'backup')
    const always = document.body.querySelector<HTMLInputElement>('input[type=checkbox]')!
    always.click()
    button('Next').click()
    await flushPromises()

    await wrapper.setProps({ open: false })
    await wrapper.setProps({ open: true })
    await flushPromises()

    expect(document.body.textContent).not.toContain('Choose a location')
    expect(document.body.textContent).toContain('location: Volume 2 (backup)')
  })
})
