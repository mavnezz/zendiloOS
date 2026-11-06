import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { CatalogApp } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import { appText, permissionKeys } from '../texts'
import StoreSection from './StoreSection.vue'

function app(id: string, name: string, extra: Partial<CatalogApp> = {}): CatalogApp {
  return {
    source_id: 1,
    official: true,
    id,
    name,
    version: '1.0',
    tagline: `${name} on the NAS`,
    categories: ['media'],
    permissions: { devices: [], capabilities: [], host_network: false },
    translations: {},
    digest: `sha256:${id}`,
    ...extra,
  }
}

const apps = ref<CatalogApp[]>([])

vi.mock('../api', () => ({
  useCatalog: () => ({ data: apps, error: ref(null), isPending: ref(false) }),
  iconUrl: (a: CatalogApp) => `/icon/${a.id}`,
  useDocker: () => ({ data: ref(undefined) }),
  useEnableDocker: () => () => Promise.resolve(),
}))

beforeEach(() => {
  i18n.global.locale.value = 'en'
  apps.value = [
    app('jellyfin', 'Jellyfin', {
      permissions: { devices: ['/dev/dri'], capabilities: [], host_network: false },
    }),
    app('immich', 'Immich', {
      categories: ['photos', 'backup'],
      translations: {
        de: { tagline: 'Fotos vom Handy sichern', name: null, description: null, notes: null },
      },
    }),
    app('paperless', 'Paperless', { official: false, categories: ['productivity'] }),
  ]
})

describe('the app store', () => {
  it('shows every app with what sets it apart', () => {
    const text = mount(StoreSection, { global: { plugins: [i18n] } }).text()

    expect(text).toContain('Jellyfin')
    expect(text).toContain('Graphics card')
    expect(text).toContain('Other catalog')
  })

  it('finds apps by name or tagline and by category', async () => {
    const wrapper = mount(StoreSection, { global: { plugins: [i18n] } })

    await wrapper.find('input').setValue('paper')
    expect(wrapper.findAll('li').map((li) => li.text())).toHaveLength(1)
    await wrapper.find('input').setValue('')
    const photos = wrapper.findAll('button').find((b) => b.text() === 'Photos')
    await photos?.trigger('click')
    await flushPromises()
    const names = wrapper.findAll('li').map((li) => li.text())
    expect(names).toHaveLength(1)
    expect(names[0]).toContain('Immich')
  })
})

describe('app texts', () => {
  it('use the interface language and fall back to English', () => {
    const immich = apps.value[1]!
    expect(appText(immich, 'tagline', 'de')).toBe('Fotos vom Handy sichern')
    expect(appText(immich, 'name', 'de')).toBe('Immich')
    expect(appText(immich, 'tagline', 'fr')).toBe('Immich on the NAS')
  })

  it('name each kind of permission once', () => {
    const asks = app('x', 'X', {
      permissions: {
        devices: ['/dev/dri', '/dev/dri/renderD128'],
        capabilities: ['NET_ADMIN'],
        host_network: true,
      },
    })
    expect(permissionKeys(asks)).toEqual(['gpu', 'device', 'netAdmin', 'hostNetwork'])
  })
})
