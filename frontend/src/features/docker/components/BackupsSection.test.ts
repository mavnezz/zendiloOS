import type { VueWrapper } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { click, mountApp, page, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type {
  BackupPlanView,
  DockerBackupGroup,
  DockerVolume,
  Job,
  VolumeBackup,
} from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatBytes, formatDateTime } from '@/shared/lib/format'

import BackupsSection from './BackupsSection.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

function backup(id: number, volume: string, extra: Partial<VolumeBackup> = {}): VolumeBackup {
  return {
    id,
    volume,
    share_id: 1,
    started_at: 1_790_000_000 + id,
    finished_at: 1_790_000_100 + id,
    state: 'succeeded',
    error: null,
    archive: `docker/${volume}/${volume}-2026110${id}-030000.tar.zst`,
    archive_bytes: 2_000_000 + id,
    manifest: `docker/${volume}/${volume}-2026110${id}-030000.json`,
    sha256: 'ab'.repeat(32),
    data_bytes: 5_000_000,
    started_by: null,
    imported: false,
    ...extra,
  }
}

const plan: BackupPlanView = {
  share_id: 1,
  folder: 'docker',
  frequency: 'daily',
  weekday: null,
  day: null,
  hour: 3,
  keep: 7,
  stop_containers: true,
  next_at: 1_790_100_000,
  paused: false,
}

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

function job(id: number, kind: string, target: string, extra: Partial<Job> = {}): Job {
  return {
    id,
    kind,
    target,
    state: 'running',
    step: null,
    progress: null,
    error_code: null,
    error_detail: null,
    actor: 'alice',
    created_at: 0,
    updated_at: 0,
    finished_at: null,
    output: [],
    ...extra,
  }
}

const groups: DockerBackupGroup[] = [
  {
    volume: 'gone_db',
    exists: false,
    plan: { ...plan, paused: true },
    backups: [
      backup(3, 'gone_db'),
      backup(2, 'gone_db', {
        state: 'failed',
        error: 'disk full',
        archive: null,
        archive_bytes: null,
      }),
    ],
  },
  {
    volume: 'lost',
    exists: false,
    plan: null,
    backups: [backup(1, 'lost', { imported: true })],
  },
  {
    volume: 'tesla_db',
    exists: true,
    plan,
    backups: [backup(4, 'tesla_db', { started_by: 'alice' })],
  },
]

async function open(): Promise<VueWrapper> {
  const { wrapper } = await mountApp(BackupsSection)
  return wrapper
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

function buttons(label: string): HTMLButtonElement[] {
  return [...document.querySelectorAll<HTMLButtonElement>(`button[aria-label="${label}"]`)]
}

function button(text: string): HTMLButtonElement {
  const found = [...document.body.querySelectorAll<HTMLButtonElement>('button')].find(
    (candidate) => candidate.textContent?.trim() === text,
  )
  if (!found) throw new Error(`no button ${text}`)
  return found
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/docker/backups', groups)
  on('GET', '/api/v1/docker/volumes', [volume('tesla_db'), volume('cache')])
  on('GET', '/api/v1/files/shares', [
    { id: 1, name: 'backups', access: 'read_write', home: false },
    { id: 2, name: 'media', access: 'read', home: false },
  ])
})

describe('the backups page', () => {
  it('lists the backups by volume, also of volumes deleted, with their plans', async () => {
    await open()
    const text = page()
    expect(text).toContain('gone_db')
    expect(text).toContain(t('docker.backups.deleted'))
    expect(text).toContain(t('docker.backups.paused'))
    expect(text).toContain(t('docker.backups.pausedGone'))
    expect(text).toContain(t('docker.backups.noPlan'))
    expect(text).toContain(t('docker.backups.imported'))
    expect(text).toContain(formatBytes(2_000_003))
    expect(text).toContain(formatDateTime(1_790_000_003 * 1000))
    expect(text).toContain('disk full')
    expect(text).toContain('alice')
    expect(text).toContain(
      `Daily at 03:00 · ${t('docker.volumes.next', { at: formatDateTime(1_790_100_000 * 1000) })}`,
    )
    // A plan is resumed once its volume is there again.
    expect(button(t('docker.backups.resume')).disabled).toBe(true)
    expect(buttons(t('docker.volumes.list.restore')).map((b) => b.disabled)).toEqual([
      false,
      true,
      false,
      false,
    ])

    await type(`input[aria-label="${t('docker.search')}"]`, 'lost')
    expect(page()).not.toContain('gone_db')
    await type(`input[aria-label="${t('docker.search')}"]`, 'nothing at all')
    expect(page()).toContain(t('docker.backups.none'))
  })

  it('restores the backup of a deleted volume under its own name', async () => {
    on('POST', '/api/v1/docker/backups/3/restore', job(6, 'containers.volume_restore', 'gone_db'))
    const wrapper = await open()
    buttons(t('docker.volumes.list.restore'))[0]!.click()
    await settle()
    expect(page()).toContain(t('docker.volumes.restore.own', { name: 'gone_db' }))
    expect(page()).toContain(t('docker.volumes.restore.ownHint'))
    await submit('#restore-backup')
    await confirm(wrapper)
    expect(called('POST', '/api/v1/docker/backups/3/restore')[0]?.body).toEqual({
      target: 'own_name',
      volume: null,
      stop_containers: false,
    })
    expect(useJobsStore().jobs.get(6)?.kind).toBe('containers.volume_restore')
  })

  it('restores into a volume that exists, chosen from those there', async () => {
    on('POST', '/api/v1/docker/backups/1/restore', job(7, 'containers.volume_restore', 'cache'))
    const wrapper = await open()
    buttons(t('docker.volumes.list.restore'))[2]!.click()
    await settle()
    document.querySelector<HTMLInputElement>('input[value=existing]')!.click()
    await settle()
    const options = [
      ...document.querySelectorAll<HTMLOptionElement>(
        `select[aria-label="${t('docker.volumes.restore.choose')}"] option:not([disabled])`,
      ),
    ].map((option) => option.value)
    expect(options).toEqual(['tesla_db', 'cache'])
    await type(`select[aria-label="${t('docker.volumes.restore.choose')}"]`, 'cache')
    await submit('#restore-backup')
    await confirm(wrapper)
    expect(called('POST', '/api/v1/docker/backups/1/restore')[0]?.body).toEqual({
      target: 'existing',
      volume: 'cache',
      stop_containers: false,
    })
  })

  it('deletes a backup by hand after asking, and says why one is refused', async () => {
    on('DELETE', '/api/v1/docker/backups/3', null)
    on('DELETE', '/api/v1/docker/backups/2', problem(409, 'volume_busy'))
    const wrapper = await open()
    buttons(t('docker.backups.remove'))[0]!.click()
    await settle()
    expect(page()).toContain(t('docker.backups.removeTitle', { name: 'gone_db' }))
    expect(page()).toContain(
      t('docker.backups.removeText', { at: formatDateTime(1_790_000_003 * 1000) }),
    )
    await confirm(wrapper)
    expect(called('DELETE', '/api/v1/docker/backups/3')).toHaveLength(1)

    buttons(t('docker.backups.remove'))[1]!.click()
    await settle()
    expect(page()).toContain(t('docker.backups.removeFailedText'))
    await confirm(wrapper)
    expect(page()).toContain(t('errors.volume_busy'))
  })

  it('deletes nothing while a backup, restore or import runs', async () => {
    await open()
    useJobsStore().update(job(9, 'containers.volume_import', 'backups/old'))
    await settle()
    expect(buttons(t('docker.backups.remove')).every((b) => b.disabled)).toBe(true)
  })

  it('resumes a paused plan once its volume is back, or deletes it', async () => {
    on('GET', '/api/v1/docker/backups', [
      { volume: 'tesla_db', exists: true, plan: { ...plan, paused: true }, backups: [] },
    ])
    on('POST', '/api/v1/docker/volumes/tesla_db/backup-plan/resume', plan)
    on('DELETE', '/api/v1/docker/volumes/tesla_db/backup-plan', null)
    const wrapper = await open()
    expect(page()).toContain(t('docker.backups.pausedBack'))
    expect(page()).toContain(t('docker.volumes.list.none'))
    await click(t('docker.backups.resume'))
    expect(called('POST', '/api/v1/docker/volumes/tesla_db/backup-plan/resume')).toHaveLength(1)
    await click(t('docker.backups.deletePlan'))
    expect(page()).toContain(t('docker.backups.deletePlanTitle', { name: 'tesla_db' }))
    await confirm(wrapper)
    expect(called('DELETE', '/api/v1/docker/volumes/tesla_db/backup-plan')).toHaveLength(1)
  })

  it('imports the backups in a folder, and shows what was recorded and refused', async () => {
    on('POST', '/api/v1/docker/backups/import', job(11, 'containers.volume_import', 'backups/old'))
    await open()
    await click(t('docker.backups.import'))
    expect(page()).toContain(t('docker.backups.importDialog.rules'))
    expect(button(t('docker.backups.importDialog.start')).disabled).toBe(true)
    await type('#import-folder', 'media/old')
    expect(button(t('docker.backups.importDialog.start')).disabled).toBe(true)
    await type('#import-folder', 'backups/old')
    await submit('#import-backups')
    expect(called('POST', '/api/v1/docker/backups/import')[0]?.body).toEqual({
      share_id: 1,
      folder: 'old',
    })
    expect(page()).toContain(t('docker.backups.importDialog.waiting'))
    expect(page()).toContain(t('docker.backups.importDialog.running'))

    useJobsStore().update(
      job(11, 'containers.volume_import', 'backups/old', {
        state: 'succeeded',
        output: [
          'Recorded old/lost/lost-20261104-030000.tar.zst',
          'Refused old/lost/lost-20261103-030000.tar.zst: the archive: writable by others',
          '1 recorded, 0 recorded before, 1 refused',
        ],
      }),
    )
    await settle()
    const output = document.querySelector('[data-testid="import-output"]')?.textContent ?? ''
    expect(output).toContain(
      'Refused old/lost/lost-20261103-030000.tar.zst: the archive: writable by others',
    )
    expect(page()).toContain(t('docker.backups.importDialog.succeeded'))
    await click(t('docker.backups.importDialog.again'))
    expect(document.querySelector<HTMLInputElement>('#import-folder')?.value).toBe('')
  })

  it('says why an import is refused', async () => {
    on('POST', '/api/v1/docker/backups/import', problem(403, 'share_denied'))
    await open()
    await click(t('docker.backups.import'))
    await type('#import-folder', 'backups')
    await submit('#import-backups')
    expect(called('POST', '/api/v1/docker/backups/import')[0]?.body).toEqual({
      share_id: 1,
      folder: '',
    })
    expect(page()).toContain(t('errors.share_denied'))
  })
})
