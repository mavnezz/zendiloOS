import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { VmPlugin } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatBytes } from '@/shared/lib/format'
import { button, click, mountApp, page, submit } from '@/shared/test-support/mount'
import { called, on, reset } from '@/shared/test-support/server'

import VmsSection from './VmsSection.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

function plugin(extra: Partial<VmPlugin> = {}): VmPlugin {
  return {
    hardware: { kvm: true, cpu_virtualization: 'svm', iommu: true },
    installed: false,
    storage: null,
    pools: [
      { pool: 'cold', volume: 2 },
      { pool: 'tank', volume: 1 },
    ],
    cores: 8,
    memory_bytes: 16 * 1024 ** 3,
    bridges: [],
    image_upload_max_bytes: 32 * 1024 ** 3,
    ...extra,
  }
}

function text(id: string): string {
  return document.querySelector(`[data-testid="${id}"]`)?.textContent ?? ''
}

beforeEach(() => {
  reset()
})

describe('the Virtual machines plugin', () => {
  it('checks the hardware and installs as a job', async () => {
    on('GET', '/api/v1/vms/plugin', plugin())
    on('POST', '/api/v1/vms/plugin/install', {
      id: 9,
      kind: 'vms.install',
      target: 'libvirt',
      state: 'running',
      step: 'packages',
      progress: 0.05,
      error_code: null,
      error_detail: null,
      created_at: 0,
      updated_at: 0,
      finished_at: null,
      output: [],
    })
    await mountApp(VmsSection)
    const flag = t('appStore.vms.flags.svm')
    expect(text('vms-kvm')).toContain(t('appStore.vms.kvmOn', { flag }))
    expect(text('vms-iommu')).toContain(t('appStore.vms.iommuOn'))
    expect(page()).toContain(
      t('appStore.vms.hostValue', { cores: 8, memory: formatBytes(16 * 1024 ** 3) }),
    )
    await click(t('appStore.install'))
    expect(called('POST', '/api/v1/vms/plugin/install')).toHaveLength(1)
    expect(useJobsStore().list[0]?.kind).toBe('vms.install')
    expect(page()).toContain(t('appStore.vms.installing'))
  })

  it('refuses to install where there is no KVM, and says why', async () => {
    on(
      'GET',
      '/api/v1/vms/plugin',
      plugin({ hardware: { kvm: false, cpu_virtualization: null, iommu: false } }),
    )
    await mountApp(VmsSection)
    expect(button(t('appStore.install')).disabled).toBe(true)
    expect(text('vms-kvm')).toContain(t('appStore.vms.kvmOff'))
    expect(text('vms-iommu')).toContain(t('appStore.vms.iommuOff'))
    expect(page()).toContain(t('appStore.vms.noKvm'))
  })

  it('puts the storage on the pool picked once installed', async () => {
    on('GET', '/api/v1/vms/plugin', plugin({ installed: true }))
    on(
      'POST',
      '/api/v1/vms/plugin/storage',
      plugin({ installed: true, storage: { pool: 'tank', volume: 1 } }),
    )
    await mountApp(VmsSection)
    expect(page()).toContain(t('appStore.vms.notSetUp'))
    await click(t('appStore.vms.choosePlace'))
    expect(document.querySelector<HTMLSelectElement>('#vms-pool')?.value).toBe('tank')
    await submit('#vms-place')
    expect(called('POST', '/api/v1/vms/plugin/storage')[0]?.body).toEqual({ pool: 'tank' })
    expect(page()).toContain(t('appStore.placeVolume', { number: 1, pool: 'tank' }))
  })
})
