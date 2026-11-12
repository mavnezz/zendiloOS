import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { WINDOW_SECTION } from '@/shared/lib/window-section'
import { click, mountApp, page, settle } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import BackupSection from './BackupSection.vue'
import PluginsSection from './PluginsSection.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)
const openApp = vi.fn()
const JOB = {
  id: 3,
  kind: 'backup.install',
  target: 'rsync',
  state: 'running',
  created_at: 1,
  updated_at: 1,
}

beforeEach(() => {
  reset()
  openApp.mockClear()
  on('GET', '/api/v1/backup/plugin', { installed: false, ssh: false, public_key: null })
})

describe('the Backup plugin', () => {
  it('installs rsync, says how it goes, then leads to its app', async () => {
    on('POST', '/api/v1/backup/plugin/install', JOB)
    await mountApp(BackupSection, {
      global: {
        provide: {
          [WINDOW_SECTION as symbol]: { section: { value: '' }, navigate: () => {}, openApp },
        },
      },
    })
    expect(page()).toContain(t('appStore.backup.notInstalled'))
    expect(page()).toContain(t('appStore.backup.aboutSecurity'))
    await click(t('appStore.install'))
    expect(called('POST', '/api/v1/backup/plugin/install')).toHaveLength(1)
    expect(page()).toContain(t('appStore.backup.installing'))

    const jobs = useJobsStore()
    on('GET', '/api/v1/backup/plugin', { installed: true, ssh: true, public_key: null })
    jobs.update({
      ...JOB,
      state: 'failed',
      error_detail: 'E: Unable to locate package rsync',
    } as never)
    await settle()
    expect(page()).toContain(t('appStore.backup.failed'))
    expect(page()).toContain('Unable to locate package rsync')
    await click(t('appStore.backup.toApp'))
    expect(openApp).toHaveBeenCalledWith('backup')
  })

  it('says why an installation did not start', async () => {
    on('POST', '/api/v1/backup/plugin/install', problem(409, 'update_running', 'system.update'))
    await mountApp(BackupSection)
    await click(t('appStore.install'))
    expect(page()).toContain(t('errors.update_running'))
  })

  it('is a card among the plugins, with its state', async () => {
    on('GET', '/api/v1/apps/docker', { installed: true, running: true, volume: null })
    on('GET', '/api/v1/certificates/plugin', { installed: false })
    on('GET', '/api/v1/vms/plugin', problem(403, 'forbidden'))
    const { wrapper } = await mountApp(PluginsSection)
    expect(page()).toContain(t('appStore.backup.name'))
    expect(page()).toContain(t('appStore.backup.notInstalled'))
    await click(new RegExp(t('appStore.backup.subtitle')))
    expect(wrapper.emitted('open')?.at(-1)).toEqual(['backup'])
  })
})
