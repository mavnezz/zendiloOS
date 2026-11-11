import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'

import type { BackupRun, BackupTask } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { WINDOW_SECTION } from '@/shared/lib/window-section'
import { button, click, mountApp, page, settle, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import BackupApp from './BackupApp.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)
vi.mock('@/shared/ui/folder-picker', () => ({
  useShareList: () => ({
    data: ref([
      { id: 1, name: 'photos', writable: true },
      { id: 2, name: 'backups', writable: true },
    ]),
  }),
  // Picks what its button names.
  FolderPicker: defineComponent({
    props: { open: Boolean, value: String, writable: Boolean },
    emits: ['pick', 'update:open'],
    setup:
      (props, { emit }) =>
      () =>
        props.open
          ? h('div', { 'data-testid': 'picker' }, [
              h(
                'button',
                { type: 'button', onClick: () => emit('pick', 'backups/restored') },
                'pick:backups/restored',
              ),
            ])
          : null,
  }),
}))

const t = (key: string, values: Record<string, unknown> | number = {}) =>
  typeof values === 'number' ? i18n.global.t(key, values) : i18n.global.t(key, values)

function run(extra: Partial<BackupRun> = {}): BackupRun {
  return {
    id: 7,
    task_id: 1,
    trigger: 'schedule',
    by: null,
    started_at: 1_790_000_000,
    finished_at: 1_790_000_060,
    outcome: 'succeeded',
    files: 3,
    bytes: 3072,
    version: '2026-09-21_120000Z',
    error: null,
    transfers: 2,
    job_id: 4,
    ...extra,
  }
}

function task(id: number, name: string, extra: Partial<BackupTask> = {}): BackupTask {
  return {
    id,
    name,
    kind: 'local',
    folder: name,
    source: { kind: 'share', share_id: 1, path: 'albums' },
    target: { kind: 'share', share_id: 2, path: '' },
    server: null,
    password_set: false,
    mode: { kind: 'versioned', keep_last: 30, keep_days: null },
    schedule: { frequency: 'daily', weekday: null, hour: 3, minute: 0 },
    on_plug: false,
    excludes: ['*.tmp'],
    hidden: true,
    bwlimit_kib: null,
    notify_success: false,
    created_at: 1_790_000_000,
    running: false,
    reachable: true,
    next_at: 1_790_100_000,
    last_run: run({ task_id: id }),
    ...extra,
  }
}

const OFFSITE = task(2, 'Offsite', {
  kind: 'push',
  target: { kind: 'server', path: 'nas' },
  server: { transport: 'daemon', host: 'nas2.lan', port: 873, user: 'zendi', module: 'backup' },
  password_set: true,
  mode: { kind: 'mirror', keep_changed_days: 7 },
  last_run: run({
    id: 8,
    task_id: 2,
    outcome: 'failed',
    error: 'rsync: failed to connect to nas2.lan',
  }),
})

const openApp = vi.fn()
const section = ref<string | undefined>(undefined)

async function mountBackup(at?: string) {
  section.value = at
  return mountApp(BackupApp, {
    global: {
      provide: {
        [WINDOW_SECTION as symbol]: {
          section,
          navigate: (next: string) => (section.value = next),
          openApp,
        },
      },
    },
  })
}

function card(id: number): string {
  return document.querySelector(`[data-task="${id}"]`)?.textContent ?? ''
}

function within(id: number, text: string): HTMLButtonElement {
  return [...document.querySelectorAll<HTMLButtonElement>(`[data-task="${id}"] button`)].find(
    (candidate) => candidate.textContent?.trim() === text,
  )!
}

/** Opens a reka menu the way a pointer does, and picks `item`. */
async function menu(label: string, item: string): Promise<void> {
  const trigger = document.querySelector(`button[aria-label="${label}"]`)!
  for (const kind of ['pointerdown', 'pointerup', 'click'])
    trigger.dispatchEvent(new MouseEvent(kind, { bubbles: true, button: 0 }))
  await settle()
  const entry = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((element) =>
    element.textContent?.includes(item),
  )
  if (!entry) throw new Error(`no menu item ${item}`)
  entry.click()
  await settle()
}

beforeEach(() => {
  reset()
  openApp.mockClear()
  on('GET', '/api/v1/backup/plugin', { installed: true, ssh: true, public_key: null })
  on('GET', '/api/v1/backup/tasks', [task(1, 'Albums'), OFFSITE])
  on('GET', '/api/v1/storage/removable', [])
  on('GET', '/api/v1/backup/settings', { log_days: 90, bwlimit_kib: 0 })
  on('GET', '/api/v1/backup/shares/2/readers', { accounts: ['alice'], groups: ['family'], nfs: [] })
})

describe('the Backup app', () => {
  it('points to the App Center until rsync is installed', async () => {
    on('GET', '/api/v1/backup/plugin', { installed: false, ssh: false, public_key: null })
    await mountBackup()
    expect(page()).toContain(t('backup.notReady'))
    expect(called('GET', '/api/v1/backup/tasks')).toHaveLength(0)
    await click(t('backup.toAppCenter'))
    expect(openApp).toHaveBeenCalledWith('apps', 'backup')
  })

  it('shows at a glance what needs attention, the last and the next run, and the targets', async () => {
    await mountBackup('overview')
    expect(page()).toContain(t('backup.overview.states.attention'))
    expect(page()).toContain('rsync: failed to connect to nas2.lan')
    expect(page()).toContain(t('backup.overview.nextRun'))
    expect(page()).toContain('backups')
    expect(page()).toContain(t('backup.overview.serverUnknown'))
    expect(document.querySelector('nav')?.textContent).toContain(t('backup.sections.tasks'))
  })

  it('says so when there is nothing yet, and leads to making a task', async () => {
    on('GET', '/api/v1/backup/tasks', [])
    await mountBackup('overview')
    expect(page()).toContain(t('backup.overview.states.empty'))
    await click(t('backup.add'))
    expect(section.value).toBe('tasks')
  })

  it('shows each task as a card, backs up now and stops', async () => {
    on('POST', '/api/v1/backup/tasks/1/run', {
      id: 9,
      kind: 'backup.run',
      target: 'Albums',
      state: 'running',
      created_at: 1,
      updated_at: 1,
    })
    on('POST', '/api/v1/backup/tasks/3/stop', null)
    on('GET', '/api/v1/backup/tasks', [
      task(1, 'Albums'),
      OFFSITE,
      task(3, 'Running', { running: true, last_run: null }),
    ])
    await mountBackup('tasks')
    expect(card(1)).toContain(t('backup.status.succeeded'))
    expect(card(1)).toContain('photos › albums')
    expect(card(1)).toContain(t('backup.modes.versioned'))
    expect(card(2)).toContain('zendi@nas2.lan::backup › nas')
    expect(card(2)).toContain(t('backup.status.failed'))
    within(1, t('backup.backUpNow')).click()
    await settle()
    expect(called('POST', '/api/v1/backup/tasks/1/run')).toHaveLength(1)
    within(3, t('backup.stop')).click()
    await settle()
    expect(called('POST', '/api/v1/backup/tasks/3/stop')).toHaveLength(1)

    on('POST', '/api/v1/backup/tasks/1/run', problem(409, 'drive_missing'))
    within(1, t('backup.backUpNow')).click()
    await settle()
    expect(page()).toContain(t('errors.drive_missing'))
  })

  it('checks a server, leads to the restore and the log, and deletes after asking', async () => {
    on('POST', '/api/v1/backup/servers/check', [
      { name: 'photos', kind: 'folder', bytes: 0, modified: null },
    ])
    on('DELETE', '/api/v1/backup/tasks/2', null)
    await mountBackup('tasks')
    await menu(t('backup.more', { name: 'Offsite' }), t('backup.actions.check'))
    expect(called('POST', '/api/v1/backup/servers/check')[0]?.body).toMatchObject({
      path: 'nas',
      task_id: 2,
      password: null,
    })
    expect(page()).toContain(t('backup.server.checkOk', 1))

    await menu(t('backup.more', { name: 'Offsite' }), t('backup.actions.delete'))
    expect(page()).toContain(t('backup.deleteText', { folder: 'Offsite' }))
    const confirm = [
      ...document.querySelectorAll<HTMLButtonElement>('[role="alertdialog"] button'),
    ].find((candidate) => candidate.textContent?.trim() === t('backup.actions.delete'))!
    confirm.click()
    await settle()
    expect(called('DELETE', '/api/v1/backup/tasks/2')).toHaveLength(1)

    on('GET', '/api/v1/backup/tasks/1/versions', { current: false, versions: [] })
    await menu(t('backup.more', { name: 'Albums' }), t('backup.actions.restore'))
    expect(section.value).toBe('restore')
  })

  it('edits a task in tabs of the wizard’s parts', async () => {
    on('PUT', '/api/v1/backup/tasks/1', task(1, 'Albums nightly'))
    await mountBackup('tasks')
    await menu(t('backup.more', { name: 'Albums' }), t('backup.actions.edit'))
    expect(page()).toContain(t('backup.wizard.editTitle', { name: 'Albums' }))
    expect(document.querySelector<HTMLInputElement>('#backup-name')?.value).toBe('Albums')
    await type('#backup-name', 'Albums nightly')
    await click(t('backup.wizard.tabs.options'))
    expect(page()).toContain(t('backup.wizard.keepLast'))
    await click(t('backup.wizard.steps.review'))
    expect(document.querySelector('[data-testid="backup-review"]')?.textContent).toContain(
      'Albums nightly',
    )
    await click(t('backup.wizard.save'))
    const body = called('PUT', '/api/v1/backup/tasks/1')[0]?.body as Record<string, unknown>
    expect(body.name).toBe('Albums nightly')
    expect(body.source).toEqual({ kind: 'share', share_id: 1, path: 'albums' })
  })

  it('restores some of a version into another folder', async () => {
    on('GET', '/api/v1/backup/tasks/1/versions', {
      current: false,
      versions: [1_790_000_000, 1_789_000_000],
    })
    on('GET', '/api/v1/backup/tasks/1/browse', (call: { query: Record<string, string> }) =>
      call.query.path === '2024'
        ? [{ name: 'beach.jpg', kind: 'file', bytes: 1024, modified: null }]
        : [
            { name: '2024', kind: 'folder', bytes: 0, modified: null },
            { name: 'notes.txt', kind: 'file', bytes: 10, modified: null },
          ],
    )
    on('POST', '/api/v1/backup/tasks/1/restore', {
      id: 10,
      kind: 'backup.restore',
      target: 'Albums',
      state: 'running',
      created_at: 1,
      updated_at: 1,
    })
    await mountBackup('restore')
    expect(called('GET', '/api/v1/backup/tasks/1/versions')).toHaveLength(1)
    const some = [...document.querySelectorAll<HTMLInputElement>('input[type="radio"]')].find(
      (input) => input.parentElement?.textContent?.includes(t('backup.restore.some')),
    )!
    some.click()
    await settle()
    expect(document.querySelector('[data-testid="restore-browser"]')?.textContent).toContain(
      'notes.txt',
    )
    document.querySelector<HTMLInputElement>('input[aria-label="notes.txt"]')!.click()
    await settle()
    await click(/^2024/)
    expect(page()).toContain('beach.jpg')
    document.querySelector<HTMLInputElement>('input[aria-label="beach.jpg"]')!.click()
    await settle()
    await click(new RegExp(t('backup.restore.up')))
    const elsewhere = [...document.querySelectorAll<HTMLInputElement>('input[type="radio"]')].find(
      (input) => input.parentElement?.textContent?.includes(t('backup.restore.elsewhere')),
    )!
    elsewhere.click()
    await settle()
    await click(new RegExp(t('backup.wizard.choose')))
    await click('pick:backups/restored')
    const replace = [...document.querySelectorAll<HTMLInputElement>('input[type="radio"]')].find(
      (input) => input.parentElement?.textContent?.includes(t('backup.restore.replace')),
    )!
    replace.click()
    await settle()
    // The page's own button, not the sidebar's entry of the same name.
    const start = [...document.querySelectorAll<HTMLButtonElement>('button')].filter(
      (candidate) => candidate.textContent?.trim() === t('backup.restore.start'),
    )
    start.at(-1)!.click()
    await settle()
    expect(called('POST', '/api/v1/backup/tasks/1/restore')[0]?.body).toEqual({
      version: 1_790_000_000,
      paths: ['2024/beach.jpg', 'notes.txt'],
      to: { kind: 'share', share_id: 2, path: 'restored' },
      existing: 'replace',
    })
    expect(page()).toContain(t('backup.restore.started'))
  })

  it('says when a task’s target holds nothing to restore', async () => {
    on('GET', '/api/v1/backup/tasks/1/versions', { current: false, versions: [] })
    await mountBackup('restore')
    expect(page()).toContain(t('backup.restore.noVersions'))
    on('GET', '/api/v1/backup/tasks/2/versions', problem(409, 'host_key_changed', 'ED25519'))
    await type('#restore-task', '2')
    expect(page()).toContain(t('errors.host_key_changed'))
  })

  it('logs every run with its transfer list', async () => {
    on('GET', '/api/v1/backup/runs', [
      run(),
      run({
        id: 8,
        task_id: 2,
        outcome: 'failed',
        error: 'Connection refused',
        by: 'alice',
        trigger: 'manual',
      }),
    ])
    on('GET', '/api/v1/backup/tasks/2/runs', [run({ id: 8, task_id: 2, outcome: 'failed' })])
    on('GET', '/api/v1/backup/runs/7/transfers', {
      run: run(),
      transfers: [
        { change: '>f+++++++++', path: 'albums/beach.jpg' },
        { change: '*deleting', path: 'old.txt' },
      ],
    })
    await mountBackup('log')
    const rows = document.querySelectorAll('[data-run]')
    expect(rows).toHaveLength(2)
    expect(rows[1]?.textContent).toContain('alice')
    expect(rows[1]?.textContent).toContain('Connection refused')
    expect(rows[0]?.textContent).toContain(t('backup.log.outcomes.succeeded'))
    const details = [...document.querySelectorAll<HTMLButtonElement>('[data-run="7"] button')][0]!
    details.click()
    await settle()
    expect(page()).toContain('albums/beach.jpg')
    expect(page()).toContain(t('backup.log.changes.deleted'))
    await type('input[aria-label="' + t('backup.log.search') + '"]', 'beach')
    expect(page()).not.toContain('old.txt')

    await type('#backup-log-task', '2')
    expect(called('GET', '/api/v1/backup/tasks/2/runs')).toHaveLength(1)
  })

  it('keeps the settings', async () => {
    on('PUT', '/api/v1/backup/settings', { log_days: 30, bwlimit_kib: 500 })
    await mountBackup('settings')
    expect(document.querySelector<HTMLInputElement>('#backup-log-days')?.value).toBe('90')
    await type('#backup-log-days', '30')
    await type('#backup-bandwidth', '500')
    await click(t('backup.settings.save'))
    expect(called('PUT', '/api/v1/backup/settings')[0]?.body).toEqual({
      log_days: 30,
      bwlimit_kib: 500,
    })
    expect(page()).toContain(t('backup.settings.saved'))
    on(
      'PUT',
      '/api/v1/backup/settings',
      problem(422, 'invalid_input', 'the log is kept 1 to 3650 days'),
    )
    await click(t('backup.settings.save'))
    expect(page()).toContain('the log is kept 1 to 3650 days')
    expect(button(t('backup.settings.save'))).toBeTruthy()
  })
})
