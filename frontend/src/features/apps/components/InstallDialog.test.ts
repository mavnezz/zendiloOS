import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { CatalogAppDetail, InstallAppRequest } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import InstallDialog from './InstallDialog.vue'

const install = vi.fn<(request: InstallAppRequest) => Promise<void>>(() => Promise.resolve())

vi.mock('../api', () => ({
  useInstallActions: () => ({ install: (request: InstallAppRequest) => install(request) }),
  useShareNames: () => ({ data: ref(['family', 'media']) }),
}))

const immich: CatalogAppDetail = {
  source_id: 1,
  official: true,
  id: 'immich',
  name: 'Immich',
  version: '2.0',
  tagline: 'Photos from your phone',
  categories: ['photos'],
  permissions: { devices: ['/dev/dri'], capabilities: [], host_network: false },
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
})

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

    expect(document.body.textContent).toContain('zendiloOS makes up: Database password')
    expect(field('port-web').value).toBe('2283')
    expect(installButton().disabled).toBe(true)

    await type('share-photos', 'family')
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
      shares: { photos: 'family' },
      settings: { ADMIN_EMAIL: 'me@example.org' },
      confirm_permissions: true,
    })
    expect(wrapper.emitted('installed')).toEqual([['immich']])
  })
})
