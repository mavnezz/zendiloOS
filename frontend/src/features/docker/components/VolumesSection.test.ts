import type { VueWrapper } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { click, mountApp, page, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type { DockerVolume, Job, VolumeBackup } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatBytes } from '@/shared/lib/format'

import { OPEN_CONTAINER } from '../lib/navigation'
import VolumesSection from './VolumesSection.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

function volume(name: string, extra: Partial<DockerVolume> = {}): DockerVolume {
  return {
    name,
    driver: 'local',
    project: null,
    labels: {},
    created_at: 1_790_000_000,
    backed_up_here: true,
    used_by: [],
    plan: null,
    last_backup: null,
    ...extra,
  }
}

function backup(id: number, extra: Partial<VolumeBackup> = {}): VolumeBackup {
  return {
    id,
    volume: 'tesla_db',
    share_id: 1,
    started_at: 1_790_000_000 + id,
    finished_at: 1_790_000_100 + id,
    state: 'succeeded',
    error: null,
    archive: `docker/tesla_db/tesla_db-2026110${id}-030000.tar.zst`,
    archive_bytes: 2_000_000,
    manifest: `docker/tesla_db/tesla_db-2026110${id}-030000.json`,
    sha256: 'ab'.repeat(32),
    data_bytes: 5_000_000,
    started_by: null,
    imported: false,
    ...extra,
  }
}

function job(id: number, kind: string, target: string): Job {
  return {
    id,
    kind,
    target,
    state: 'running',
    step: 'backing_up',
    progress: 0.42,
    error_code: null,
    error_detail: null,
    actor: 'alice',
    created_at: 0,
    updated_at: 0,
    finished_at: null,
    output: [],
  }
}

const plan = {
  share_id: 1,
  folder: 'docker',
  frequency: 'weekly' as const,
  weekday: 6,
  day: null,
  hour: 3,
  keep: 7,
  stop_containers: true,
  next_at: 1_790_100_000,
  paused: false,
}

const volumes: DockerVolume[] = [
  volume('tesla_db', {
    project: 'tesla',
    used_by: [
      { name: 'tesla-db-1', running: true },
      { name: 'tesla-backup', running: false },
    ],
    plan,
    last_backup: backup(1),
  }),
  volume('cache', { last_backup: backup(2, { state: 'failed', error: 'disk full' }) }),
  volume('media_nfs', { backed_up_here: false }),
]

const opened = vi.fn()

async function open(): Promise<VueWrapper> {
  const { wrapper } = await mountApp(VolumesSection, {
    global: { provide: { [OPEN_CONTAINER as symbol]: opened } },
  })
  return wrapper
}

async function choose(wrapper: VueWrapper, row: number, action: string): Promise<void> {
  wrapper.findAllComponents({ name: 'ActionMenu' })[row]!.vm.$emit('select', action)
  await settle()
}

async function confirm(wrapper: VueWrapper): Promise<void> {
  const shown = wrapper
    .findAllComponents({ name: 'ConfirmDialog' })
    .filter((dialog) => dialog.props('open'))
    .at(-1)
  if (!shown) throw new Error('no open confirmation')
  shown.vm.$emit('confirm')
  await settle()
}

beforeEach(() => {
  reset()
  opened.mockClear()
  on('GET', '/api/v1/docker/volumes', volumes)
  on('GET', '/api/v1/docker/volumes/sizes', [
    { name: 'tesla_db', bytes: 5_000_000, complete: true },
    { name: 'cache', bytes: 7_000_000, complete: false },
  ])
  on('GET', '/api/v1/files/shares', [
    { id: 1, name: 'backups', access: 'read_write', home: false },
    { id: 2, name: 'media', access: 'read', home: false },
  ])
  on('GET', '/api/v1/docker/backup-shares/1/readers', {
    accounts: ['alice', 'bob'],
    groups: ['family'],
    nfs: ['192.168.1.0/24'],
  })
})

describe('the volumes page', () => {
  it('lists the volumes with who uses them, their size and how they are backed up', async () => {
    await open()
    const text = page()
    expect(text).toContain('tesla_db')
    expect(text).toContain('tesla')
    expect(text).toContain(`tesla-backup (${t('docker.volumes.stopped')})`)
    expect(text).toContain(formatBytes(5_000_000))
    expect(text).toContain(t('docker.volumes.atLeast', { size: formatBytes(7_000_000) }))
    expect(text).toContain('Sundays at 03:00')
    expect(text).toContain(t('docker.volumes.noBackup'))
    expect(text).toContain(t('docker.volumes.elsewhere'))
    expect(text).toContain(t('docker.volumes.unused'))
    await click('tesla-db-1')
    expect(opened).toHaveBeenCalledWith('tesla-db-1')

    await type(`input[aria-label="${t('docker.search')}"]`, 'cach')
    expect(page()).not.toContain('tesla_db')
    await type(`input[aria-label="${t('docker.search')}"]`, 'nothing at all')
    expect(page()).toContain(t('docker.volumes.none'))
  })

  it('shows a backup under way with its step', async () => {
    const wrapper = await open()
    useJobsStore().update(job(9, 'containers.volume_backup', 'tesla_db'))
    await settle()
    expect(page()).toContain(`${t('docker.volumes.steps.backing_up')} (42 %)`)
    const actions = wrapper.findAllComponents({ name: 'ActionMenu' })[0]!.props('actions') as {
      id: string
      disabled?: boolean
    }[]
    expect(actions.find((action) => action.id === 'now')?.disabled).toBe(true)
  })

  it('removes an unused volume and all unused ones after asking', async () => {
    on('DELETE', '/api/v1/docker/volumes/cache', null)
    on('POST', '/api/v1/docker/volumes/prune', { text: 'Total reclaimed space: 7MB\n' })
    const wrapper = await open()
    const used = wrapper.findAllComponents({ name: 'ActionMenu' })[0]!.props('actions') as {
      id: string
      disabled?: boolean
    }[]
    expect(used.find((action) => action.id === 'remove')?.disabled).toBe(true)
    await choose(wrapper, 1, 'remove')
    expect(page()).toContain(t('docker.volumes.removeTitle', { name: 'cache' }))
    await confirm(wrapper)
    expect(called('DELETE', '/api/v1/docker/volumes/cache')).toHaveLength(1)

    await click(new RegExp(t('docker.volumes.prune')))
    expect(page()).toContain(t('docker.volumes.pruneText'))
    await confirm(wrapper)
    expect(called('POST', '/api/v1/docker/volumes/prune')).toHaveLength(1)
  })

  it('sets up a plan into a folder of a share', async () => {
    on('PUT', '/api/v1/docker/volumes/cache/backup-plan', { ...plan, folder: 'docker/volumes' })
    const wrapper = await open()
    await choose(wrapper, 1, 'plan')
    expect(page()).toContain(t('docker.volumes.plan.title', { name: 'cache' }))
    // Another plan takes three already: a new one starts at another hour.
    expect(document.querySelector<HTMLSelectElement>('#backup-hour')?.value).not.toBe('3')
    expect(button(t('docker.volumes.plan.save')).disabled).toBe(true)
    await type('#backup-folder', 'media/x')
    expect(button(t('docker.volumes.plan.save')).disabled).toBe(true)
    expect(page()).not.toContain(t('docker.volumes.plan.readable'))
    await type('#backup-folder', 'backups/docker/volumes')
    // Whoever may read the folder reads the volume's contents: said, with who.
    expect(page()).toContain(t('docker.volumes.plan.readable'))
    expect(page()).toContain(
      t('docker.volumes.plan.readers', {
        names: `alice, bob, ${t('docker.volumes.plan.group', { name: 'family' })}`,
      }),
    )
    expect(page()).toContain(t('docker.volumes.plan.nfs', { networks: '192.168.1.0/24' }))
    await type('#backup-frequency', 'monthly')
    await type('#backup-day', '31')
    await type('#backup-hour', '2')
    await type('#backup-keep', '3')
    await submit('#backup-plan')
    expect(called('PUT', '/api/v1/docker/volumes/cache/backup-plan')[0]?.body).toEqual({
      share_id: 1,
      folder: 'docker/volumes',
      frequency: 'monthly',
      weekday: null,
      day: 31,
      hour: 2,
      keep: 3,
      stop_containers: true,
    })
    expect(page()).not.toContain(t('docker.volumes.plan.title', { name: 'cache' }))
  })

  it('changes a plan, says why it is refused, and ends it', async () => {
    on('PUT', '/api/v1/docker/volumes/tesla_db/backup-plan', problem(403, 'share_denied'))
    on('DELETE', '/api/v1/docker/volumes/tesla_db/backup-plan', null)
    const wrapper = await open()
    await choose(wrapper, 0, 'plan')
    const folder = document.querySelector<HTMLInputElement>('#backup-folder')
    expect(folder?.value).toBe('backups/docker')
    expect(document.querySelector<HTMLSelectElement>('#backup-weekday')?.value).toBe('6')
    await submit('#backup-plan')
    expect(page()).toContain(t('errors.share_denied'))
    await click(t('docker.volumes.plan.end'))
    await confirm(wrapper)
    expect(called('DELETE', '/api/v1/docker/volumes/tesla_db/backup-plan')).toHaveLength(1)
  })

  it('backs up at once', async () => {
    on(
      'POST',
      '/api/v1/docker/volumes/tesla_db/backups',
      job(5, 'containers.volume_backup', 'tesla_db'),
    )
    on('POST', '/api/v1/docker/volumes/cache/backups', problem(409, 'volume_busy'))
    const wrapper = await open()
    await choose(wrapper, 0, 'now')
    expect(called('POST', '/api/v1/docker/volumes/tesla_db/backups')).toHaveLength(1)
    expect(useJobsStore().jobs.get(5)?.kind).toBe('containers.volume_backup')
    await choose(wrapper, 1, 'now')
    expect(page()).toContain(t('errors.volume_busy'))
  })

  it('lists a volume’s backups and restores one under its own name, its users stopped', async () => {
    on('GET', '/api/v1/docker/volumes/tesla_db/backups', [
      backup(3, { started_by: 'alice' }),
      backup(2, { state: 'failed', error: 'disk full', archive: null, archive_bytes: null }),
    ])
    on('POST', '/api/v1/docker/backups/3/restore', job(6, 'containers.volume_restore', 'tesla_db'))
    const wrapper = await open()
    await choose(wrapper, 0, 'backups')
    expect(page()).toContain(t('docker.volumes.list.title', { name: 'tesla_db' }))
    expect(page()).toContain('disk full')
    expect(page()).toContain('alice')
    const restore = [
      ...document.querySelectorAll<HTMLButtonElement>(
        `button[aria-label="${t('docker.volumes.list.restore')}"]`,
      ),
    ]
    expect(restore.map((button) => button.disabled)).toEqual([false, true])
    restore[0]!.click()
    await settle()
    expect(page()).toContain(
      t('docker.volumes.restore.inUse', { name: 'tesla_db', names: 'tesla-db-1' }),
    )
    await submit('#restore-backup')
    expect(page()).toContain(t('docker.volumes.restore.confirmTitle', { name: 'tesla_db' }))
    await confirm(wrapper)
    expect(called('POST', '/api/v1/docker/backups/3/restore')[0]?.body).toEqual({
      target: 'own_name',
      volume: null,
      stop_containers: true,
    })
    expect(useJobsStore().jobs.get(6)?.kind).toBe('containers.volume_restore')
  })

  it('restores into another volume, and not while its users may not stop', async () => {
    on('GET', '/api/v1/docker/volumes/tesla_db/backups', [backup(3)])
    on('POST', '/api/v1/docker/backups/3/restore', problem(409, 'volume_in_use', 'tesla-db-1'))
    const wrapper = await open()
    await choose(wrapper, 0, 'backups')
    document
      .querySelector<HTMLButtonElement>(`button[aria-label="${t('docker.volumes.list.restore')}"]`)!
      .click()
    await settle()
    const stop = document.querySelector<HTMLInputElement>('#restore-backup input[type=checkbox]')!
    stop.click()
    await settle()
    expect(button(t('docker.volumes.restore.start')).disabled).toBe(true)
    const existing = document.querySelector<HTMLInputElement>('input[value=existing]')!
    existing.click()
    await settle()
    expect(button(t('docker.volumes.restore.start')).disabled).toBe(true)
    await type(`select[aria-label="${t('docker.volumes.restore.choose')}"]`, 'cache')
    expect(page()).not.toContain(t('docker.volumes.restore.stop'))
    await submit('#restore-backup')
    expect(page()).toContain(t('docker.volumes.restore.confirmTitle', { name: 'cache' }))
    await confirm(wrapper)
    expect(called('POST', '/api/v1/docker/backups/3/restore')[0]?.body).toEqual({
      target: 'existing',
      volume: 'cache',
      stop_containers: false,
    })
    expect(page()).toContain(`${t('errors.volume_in_use')} (tesla-db-1)`)
  })

  it('shows a paused plan, resumes it, and backs up only then', async () => {
    on('GET', '/api/v1/docker/volumes', [
      volume('tesla_db', { plan: { ...plan, paused: true }, last_backup: backup(1) }),
    ])
    on('POST', '/api/v1/docker/volumes/tesla_db/backup-plan/resume', plan)
    const wrapper = await open()
    expect(page()).toContain(t('docker.volumes.paused'))
    const actions = wrapper.findAllComponents({ name: 'ActionMenu' })[0]!.props('actions') as {
      id: string
      disabled?: boolean
    }[]
    expect(actions.find((action) => action.id === 'now')?.disabled).toBe(true)
    await choose(wrapper, 0, 'resume')
    expect(called('POST', '/api/v1/docker/volumes/tesla_db/backup-plan/resume')).toHaveLength(1)
  })

  it('backs up from the list of backups too', async () => {
    on('GET', '/api/v1/docker/volumes/tesla_db/backups', [])
    on('POST', '/api/v1/docker/volumes/tesla_db/backups', problem(404, 'not_found'))
    const wrapper = await open()
    await choose(wrapper, 0, 'backups')
    expect(page()).toContain(t('docker.volumes.list.none'))
    await click(t('docker.volumes.backUpNow'))
    expect(called('POST', '/api/v1/docker/volumes/tesla_db/backups')).toHaveLength(1)
    on(
      'POST',
      '/api/v1/docker/volumes/tesla_db/backups',
      job(7, 'containers.volume_backup', 'tesla_db'),
    )
    await click(t('docker.volumes.backUpNow'))
    expect(useJobsStore().jobs.get(7)).toBeDefined()
  })
})

function button(text: string): HTMLButtonElement {
  const found = [...document.body.querySelectorAll<HTMLButtonElement>('button')].find(
    (candidate) => candidate.textContent?.trim() === text,
  )
  if (!found) throw new Error(`no button ${text}`)
  return found
}
