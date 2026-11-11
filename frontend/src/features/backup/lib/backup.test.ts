import { describe, expect, it } from 'vitest'

import type { BackupRun, BackupTask } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import {
  DEFAULT_EXCLUDES,
  addPattern,
  duration,
  canEncrypt,
  emptyForm,
  emptyServer,
  folderName,
  formOf,
  isFolderPath,
  isHost,
  isPattern,
  isWord,
  joinPath,
  nested,
  overview,
  parentPath,
  parseTime,
  problems,
  request,
  restorePaths,
  statusOf,
  timeOf,
  usesRclone,
  withKind,
} from './backup'
import {
  changeKind,
  placeText,
  retentionText,
  scheduleText,
  serverName,
  serverOf,
} from './describe'

const SHARE_IDS = new Map([
  ['photos', 1],
  ['backups', 2],
])
const SHARE_NAMES = new Map([
  [1, 'photos'],
  [2, 'backups'],
])
const KEY = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl'

function run(extra: Partial<BackupRun> = {}): BackupRun {
  return {
    id: 1,
    task_id: 1,
    trigger: 'schedule',
    by: null,
    started_at: 100,
    finished_at: 160,
    outcome: 'succeeded',
    files: 3,
    bytes: 300,
    version: null,
    error: null,
    transfers: 3,
    job_id: null,
    ...extra,
  }
}

function task(extra: Partial<BackupTask> = {}): BackupTask {
  return {
    id: 1,
    name: 'Photos',
    kind: 'local',
    folder: 'Photos',
    source: { kind: 'share', share_id: 1, path: 'albums' },
    target: { kind: 'share', share_id: 2, path: '' },
    server: null,
    password_set: false,
    encrypted: false,
    mode: { kind: 'versioned', keep_last: 30, keep_days: null },
    schedule: { frequency: 'daily', weekday: null, hour: 3, minute: 0 },
    on_plug: false,
    excludes: ['*.tmp'],
    hidden: true,
    bwlimit_kib: null,
    notify_success: false,
    created_at: 1,
    running: false,
    reachable: true,
    next_at: 1000,
    last_run: run(),
    ...extra,
  }
}

describe('a task form', () => {
  it('starts with the defaults: versions, daily at three, the usual patterns left out', () => {
    const form = emptyForm()
    expect(form.kind).toBe('local')
    expect(form.mode).toBe('versioned')
    expect(form.excludes).toEqual(DEFAULT_EXCLUDES)
    expect(problems(form, [])).toEqual(['name', 'source', 'target'])
    expect(emptyServer('daemon').port).toBe(873)
    expect(emptyServer().port).toBe(22)
  })

  it('becomes a local task’s request with share ids and the schedule', () => {
    const form = { ...emptyForm(), name: ' Photos ', source: 'photos/albums', target: 'backups' }
    expect(problems(form, ['other'])).toEqual([])
    expect(request(form, SHARE_IDS)).toEqual({
      name: 'Photos',
      source: { kind: 'share', share_id: 1, path: 'albums' },
      target: { kind: 'share', share_id: 2, path: '' },
      server: null,
      password: null,
      mode: { kind: 'versioned', keep_last: 30, keep_days: null },
      schedule: { frequency: 'daily', weekday: null, hour: 3, minute: 0 },
      on_plug: false,
      excludes: DEFAULT_EXCLUDES,
      hidden: true,
      bwlimit_kib: null,
      notify_success: false,
      encrypted: false,
      crypt_password: null,
      crypt_salt: null,
    })
    expect(problems({ ...form, name: 'PHOTOS' }, ['photos'])).toEqual(['nameTaken'])
    expect(problems({ ...form, target: 'photos/albums/old' }, [])).toEqual(['nested'])
  })

  it('pushes to a daemon with its password, or over SSH with the host key pinned', () => {
    const daemon = {
      ...withKind(emptyForm(), 'push'),
      name: 'Offsite',
      source: 'photos',
      server: {
        ...emptyServer('daemon'),
        host: 'nas2.lan',
        user: 'zendi',
        module: 'backup',
        password: 's3cret',
      },
      serverPath: 'nas',
      mode: 'mirror' as const,
      bandwidth: 'none' as const,
    }
    expect(problems(daemon, [])).toEqual([])
    const body = request(daemon, SHARE_IDS)
    expect(body.target).toEqual({ kind: 'server', path: 'nas' })
    expect(body.server).toEqual({
      transport: 'daemon',
      host: 'nas2.lan',
      port: 873,
      user: 'zendi',
      module: 'backup',
    })
    expect(body.password).toBe('s3cret')
    expect(body.mode).toEqual({ kind: 'mirror', keep_changed_days: 30 })
    expect(body.bwlimit_kib).toBe(0)

    const ssh = { ...daemon, server: { ...emptyServer('ssh'), host: 'nas2.lan', user: 'backup' } }
    expect(problems(ssh, [])).toEqual(['hostKey'])
    const pinned = { ...ssh, server: { ...ssh.server, hostKey: KEY, fingerprint: 'SHA256:x' } }
    expect(problems(pinned, [])).toEqual([])
    expect(request(pinned, SHARE_IDS).server).toEqual({
      transport: 'ssh',
      host: 'nas2.lan',
      port: 22,
      user: 'backup',
      host_key: KEY,
      fingerprint: 'SHA256:x',
    })
    expect(request(pinned, SHARE_IDS).password).toBeNull()
    const bad = {
      ...pinned,
      server: { ...pinned.server, host: '-oProxyCommand=x', port: 0, user: '' },
      serverPath: '-e',
    }
    expect(problems(bad, [])).toEqual(['host', 'port', 'user', 'serverPath'])
    expect(problems({ ...daemon, server: { ...daemon.server, module: '' } }, [])).toEqual([
      'module',
    ])
  })

  it('pulls a server into a drive, weekly, when the drive is plugged in', () => {
    const form = {
      ...withKind(emptyForm(), 'pull'),
      name: 'Web',
      server: { ...emptyServer(), host: 'www.lan', user: 'nas', hostKey: KEY, fingerprint: 'f' },
      serverPath: '/srv/www',
      targetKind: 'drive' as const,
      drive: '1A2B-3C4D',
      drivePath: 'servers',
      frequency: 'weekly' as const,
      weekday: 5,
      time: '22:15',
      onPlug: true,
      bandwidth: 'custom' as const,
      bwlimitKib: 500,
      keepLastOn: false,
      keepDaysOn: true,
      keepDays: 14,
    }
    expect(problems(form, [])).toEqual([])
    const body = request(form, SHARE_IDS)
    expect(body.source).toEqual({ kind: 'server', path: '/srv/www' })
    expect(body.target).toEqual({ kind: 'drive', drive: '1A2B-3C4D', path: 'servers' })
    expect(body.schedule).toEqual({ frequency: 'weekly', weekday: 5, hour: 22, minute: 15 })
    expect(body.on_plug).toBe(true)
    expect(body.mode).toEqual({ kind: 'versioned', keep_last: null, keep_days: 14 })
    expect(body.bwlimit_kib).toBe(500)
    expect(withKind(form, 'push').onPlug).toBe(false)
    expect(problems({ ...form, drive: '' }, [])).toEqual(['drive'])
    const hourly = request({ ...form, frequency: 'hourly', scheduled: true }, SHARE_IDS).schedule
    expect(hourly).toEqual({ frequency: 'hourly', weekday: null, hour: 0, minute: 15 })
    expect(request({ ...form, scheduled: false }, SHARE_IDS).schedule).toBeNull()
  })

  it('goes to a cloud account, encrypted with a password given twice', () => {
    const cloud = { ...withKind(emptyForm(), 'cloud'), name: 'To Drive', source: 'photos/albums' }
    expect(problems(cloud, [])).toEqual(['account'])
    const chosen = { ...cloud, account: 3, encrypted: true, cryptPassword: 'short' }
    expect(problems(chosen, [])).toEqual(['cryptPassword'])
    const typed = { ...chosen, cryptPassword: 'correct horse', cryptConfirm: 'correct hors' }
    expect(problems(typed, [])).toEqual(['cryptConfirm'])
    const ready = { ...typed, cryptConfirm: 'correct horse', cryptSalt: 'pepper' }
    expect(problems(ready, [])).toEqual([])
    expect(problems({ ...ready, cloudPath: '/root' }, [])).toEqual(['cloudPath'])
    expect(usesRclone(ready) && canEncrypt(ready)).toBe(true)
    const body = request(ready, SHARE_IDS)
    expect(body).toMatchObject({
      source: { kind: 'share', share_id: 1, path: 'albums' },
      target: { kind: 'cloud', account_id: 3, path: 'zendiloOS' },
      server: null,
      password: null,
      on_plug: false,
      encrypted: true,
      crypt_password: 'correct horse',
      crypt_salt: 'pepper',
    })
    // A daemon is never encrypted; a pull neither.
    const daemon = {
      ...withKind(ready, 'push'),
      server: { ...emptyServer('daemon'), host: 'h', user: 'u', module: 'm' },
    }
    expect(canEncrypt(daemon)).toBe(false)
    expect(request(daemon, SHARE_IDS)).toMatchObject({ encrypted: false, crypt_password: null })
    expect(canEncrypt(withKind(ready, 'pull'))).toBe(false)
    // Kept: the password stays as it is, unasked.
    const kept = formOf(
      task({
        kind: 'cloud',
        target: { kind: 'cloud', account_id: 3, path: 'nas' },
        encrypted: true,
      }),
      SHARE_NAMES,
    )
    expect([kept.kind, kept.account, kept.cloudPath, kept.encrypted, kept.cryptKept]).toEqual([
      'cloud',
      3,
      'nas',
      true,
      true,
    ])
    expect(problems(kept, [])).toEqual([])
    expect(request(kept, SHARE_IDS)).toMatchObject({ encrypted: true, crypt_password: null })
  })

  it('names what is out of range', () => {
    const base = { ...emptyForm(), name: 'x', source: 'photos', target: 'backups/p' }
    expect(problems({ ...base, keepLast: 0 }, [])).toEqual(['retention'])
    expect(problems({ ...base, keepLastOn: false, keepDaysOn: true, keepDays: 99999 }, [])).toEqual(
      ['retention'],
    )
    expect(problems({ ...base, mode: 'mirror', keepChangedDays: 1.5 }, [])).toEqual(['retention'])
    expect(
      problems({ ...base, mode: 'mirror', keepChanged: false, keepChangedDays: 0 }, []),
    ).toEqual([])
    expect(problems({ ...base, time: '25:00' }, [])).toEqual(['time'])
    expect(problems({ ...base, scheduled: false, time: 'x' }, [])).toEqual([])
    expect(problems({ ...base, excludes: ['- *'] }, [])).toEqual(['excludes'])
    expect(problems({ ...base, bandwidth: 'custom', bwlimitKib: 0 }, [])).toEqual(['bandwidth'])
    expect(problems({ ...base, name: 'x'.repeat(65) }, [])).toEqual(['name'])
  })

  it('comes back from a task kept, its password never', () => {
    const kept = task({
      kind: 'push',
      target: { kind: 'server', path: 'nas' },
      server: { transport: 'daemon', host: 'nas2.lan', port: 873, user: 'zendi', module: 'backup' },
      password_set: true,
      mode: { kind: 'mirror', keep_changed_days: null },
      schedule: { frequency: 'weekly', weekday: 2, hour: 4, minute: 30 },
      bwlimit_kib: 0,
    })
    const form = formOf(kept, SHARE_NAMES)
    expect(form.source).toBe('photos/albums')
    expect(form.serverPath).toBe('nas')
    expect(form.server.password).toBe('')
    expect(form.server.module).toBe('backup')
    expect([form.mode, form.keepChanged]).toEqual(['mirror', false])
    expect([form.frequency, form.weekday, form.time]).toEqual(['weekly', 2, '04:30'])
    expect(form.bandwidth).toBe('none')
    // Back as it was.
    const body = request({ ...form }, SHARE_IDS)
    expect(body.target).toEqual(kept.target)
    expect(body.schedule).toEqual(kept.schedule)

    const drive = formOf(
      task({
        target: { kind: 'drive', drive: 'D', path: 'p' },
        on_plug: true,
        schedule: null,
        bwlimit_kib: 2048,
        mode: { kind: 'versioned', keep_last: null, keep_days: 7 },
      }),
      SHARE_NAMES,
    )
    expect([drive.targetKind, drive.drive, drive.drivePath, drive.onPlug, drive.scheduled]).toEqual(
      ['drive', 'D', 'p', true, false],
    )
    expect([drive.bandwidth, drive.bwlimitKib, drive.keepLastOn, drive.keepDays]).toEqual([
      'custom',
      2048,
      false,
      7,
    ])
    const ssh = formOf(
      task({
        kind: 'pull',
        source: { kind: 'server', path: '/srv' },
        server: {
          transport: 'ssh',
          host: 'h',
          port: 22,
          user: 'u',
          host_key: KEY,
          fingerprint: 'f',
        },
      }),
      SHARE_NAMES,
    )
    expect([ssh.server.hostKey, ssh.serverPath, ssh.source]).toEqual([KEY, '/srv', ''])
    expect(
      formOf(task({ target: { kind: 'share', share_id: 2, path: '' } }), SHARE_NAMES).target,
    ).toBe('backups')
  })
})

describe('checks', () => {
  it('take what rsync and the NAS take', () => {
    expect(isPattern('*.tmp') && isPattern('@eaDir') && isPattern('/cache')).toBe(true)
    for (const bad of ['', ' x', '+ x', '- x', '!', 'a\nb', 'x'.repeat(256)])
      expect(isPattern(bad), bad).toBe(false)
    expect(addPattern(['a'], ' b ')).toEqual(['a', 'b'])
    expect(addPattern(['a'], 'a')).toEqual(['a'])
    expect(addPattern(['a'], '  ')).toEqual(['a'])
    expect(isHost('fe80::1') && isHost('nas2.lan') && !isHost('-x') && !isHost('a b')).toBe(true)
    expect(isWord('backup_1') && !isWord('-e') && !isWord('')).toBe(true)
    expect(
      isFolderPath('') && isFolderPath('a/b') && !isFolderPath('a/../b') && !isFolderPath('/a'),
    ).toBe(true)
    expect(isFolderPath('/srv', true) && !isFolderPath('-e', true) && isFolderPath('/', true)).toBe(
      true,
    )
    expect(parseTime('3:05')).toEqual({ hour: 3, minute: 5 })
    expect(parseTime('24:00')).toBeNull()
    expect(timeOf(7, 5)).toBe('07:05')
    expect(
      nested('photos', 'photos/old') &&
        nested('photos/old', 'photos') &&
        !nested('photos/a', 'photos/ab'),
    ).toBe(true)
    expect(nested('photos/a', 'backups/a')).toBe(false)
    expect(folderName('Photos to tank')).toBe('Photos to tank')
    expect(folderName('a/b\\c')).toBe('a_b_c')
    expect(folderName('..x. ')).toBe('x')
    expect(folderName('...')).toBe('backup')
  })
})

describe('how tasks stand', () => {
  it('in a word each, and at a glance', () => {
    expect(statusOf(task())).toBe('succeeded')
    expect(statusOf(task({ running: true }))).toBe('running')
    expect(statusOf(task({ reachable: false }))).toBe('unreachable')
    expect(statusOf(task({ last_run: null }))).toBe('never')
    expect(statusOf(task({ last_run: run({ outcome: 'partial' }) }))).toBe('partial')
    expect(statusOf(task({ last_run: run({ outcome: 'stopped' }) }))).toBe('failed')
    expect(statusOf(task({ last_run: run({ outcome: 'running' }) }))).toBe('running')

    expect(overview([]).state).toBe('empty')
    const fine = overview([
      task(),
      task({ id: 2, next_at: 500, last_run: run({ started_at: 300 }) }),
    ])
    expect(fine.state).toBe('fine')
    expect(fine.next?.at).toBe(500)
    expect(fine.last?.run.started_at).toBe(300)
    expect(overview([task(), task({ id: 2, running: true })]).state).toBe('running')
    const failing = overview([
      task({ last_run: run({ outcome: 'failed' }) }),
      task({ id: 2, next_at: null, last_run: null }),
    ])
    expect(failing.state).toBe('attention')
    expect(failing.attention.map((t) => t.id)).toEqual([1])
  })

  it('times runs and walks folders', () => {
    expect(duration(run())).toBe(60)
    expect(duration(run({ finished_at: null }))).toBeNull()
    expect(joinPath('', 'a')).toBe('a')
    expect(joinPath('a', 'b')).toBe('a/b')
    expect(parentPath('a/b')).toBe('a')
    expect(parentPath('a')).toBe('')
    expect(restorePaths(['a/b', 'a', 'c', 'c'])).toEqual(['a', 'c'])
  })
})

describe('words', () => {
  const t = i18n.global.t as (key: string, values?: Record<string, unknown> | number) => string
  it('name places, servers, schedules and retention', () => {
    i18n.global.locale.value = 'en'
    const daemon = {
      transport: 'daemon' as const,
      host: 'nas2.lan',
      port: 874,
      user: 'zendi',
      module: 'm',
    }
    expect(serverName(daemon)).toBe('zendi@nas2.lan:874::m')
    expect(serverName({ transport: 'ssh', host: 'fe80::1', port: 22, user: 'u' })).toBe(
      'u@[fe80::1]',
    )
    expect(placeText(t, { kind: 'share', share_id: 1, path: 'albums' }, null, SHARE_NAMES)).toBe(
      'photos › albums',
    )
    expect(placeText(t, { kind: 'share', share_id: 9, path: '' }, null, SHARE_NAMES)).toBe('#9')
    expect(
      placeText(
        t,
        { kind: 'drive', drive: 'D', path: '' },
        null,
        SHARE_NAMES,
        new Map([['D', 'STICK']]),
      ),
    ).toBe('Drive STICK')
    expect(placeText(t, { kind: 'drive', drive: 'D', path: 'x' }, null, SHARE_NAMES)).toBe(
      'Drive D › x',
    )
    expect(placeText(t, { kind: 'server', path: 'nas' }, daemon, SHARE_NAMES)).toBe(
      'zendi@nas2.lan:874::m › nas',
    )
    expect(placeText(t, { kind: 'server', path: '' }, null, SHARE_NAMES)).toBe('?')
    const accounts = new Map([[3, 'Family Drive']])
    expect(
      placeText(
        t,
        { kind: 'cloud', account_id: 3, path: 'nas' },
        null,
        SHARE_NAMES,
        new Map(),
        accounts,
      ),
    ).toBe('Family Drive › nas')
    expect(placeText(t, { kind: 'cloud', account_id: 4, path: '' }, null, SHARE_NAMES)).toBe('#4')
    expect(scheduleText(t, null, false)).toBe('By hand')
    expect(scheduleText(t, null, true)).toBe('when its drive is plugged in')
    expect(scheduleText(t, { frequency: 'hourly', minute: 5 }, false)).toBe('Hourly at :05')
    expect(scheduleText(t, { frequency: 'daily', hour: 3, minute: 0 }, true)).toBe(
      'Daily at 03:00, and when its drive is plugged in',
    )
    expect(scheduleText(t, { frequency: 'weekly', weekday: 6, hour: 22, minute: 30 }, false)).toBe(
      'Sunday at 22:30',
    )
    expect(retentionText(t, { kind: 'mirror', keep_changed_days: 7 })).toContain('7 days')
    expect(retentionText(t, { kind: 'mirror', keep_changed_days: null })).toBe(
      'Nothing a run deletes or replaces',
    )
    expect(retentionText(t, { kind: 'versioned', keep_last: 3, keep_days: 9 })).toBe(
      'The newest 3, Younger than 9 days',
    )
    expect(retentionText(t, { kind: 'versioned', keep_last: null, keep_days: null })).toBe(
      'Every version',
    )
    expect([
      changeKind('*deleting'),
      changeKind('cd+++++++++'),
      changeKind('>f+++++++++'),
      changeKind('>f.st......'),
      changeKind('.f...p.....'),
    ]).toEqual(['deleted', 'folder', 'new', 'changed', 'other'])
    expect(serverOf(task())).toBeNull()
  })
})
