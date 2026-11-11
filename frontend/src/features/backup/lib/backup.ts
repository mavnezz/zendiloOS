import type {
  BackupPlace,
  BackupRun,
  BackupSchedule,
  BackupServer,
  BackupTask,
  BackupTaskRequest,
} from '@/shared/api/types'

/** From a share to the NAS, to a server, or from a server into the NAS. */
export type TaskKind = 'local' | 'push' | 'pull'
export type Transport = 'daemon' | 'ssh'
export type Frequency = 'hourly' | 'daily' | 'weekly'
export type Bandwidth = 'default' | 'none' | 'custom'

/** What a new task leaves out unless told otherwise. */
export const DEFAULT_EXCLUDES = [
  '*.tmp',
  'Thumbs.db',
  '.DS_Store',
  '@eaDir',
  '.recycle',
  '#recycle',
  '$RECYCLE.BIN',
]
export const MAX_EXCLUDES = 64
export const MAX_KEEP_VERSIONS = 1000
export const MAX_KEEP_DAYS = 3650
export const MAX_BWLIMIT_KIB = 10_000_000
export const DAEMON_PORT = 873
export const SSH_PORT = 22

export interface ServerForm {
  transport: Transport
  host: string
  port: number
  user: string
  module: string
  /** Typed anew; empty keeps a stored one for the same daemon. */
  password: string
  /** The host key compared and pinned, with its fingerprint. */
  hostKey: string | null
  fingerprint: string | null
}

/** A task as its dialogs edit it. Places on the NAS as the folder picker names them: `share/folder`. */
export interface TaskForm {
  name: string
  kind: TaskKind
  /** For a task from the NAS: `share/folder`. */
  source: string
  /** For a task into the NAS: a share's folder, or a drive's. */
  targetKind: 'share' | 'drive'
  target: string
  drive: string
  drivePath: string
  server: ServerForm
  /** The folder on the server. */
  serverPath: string
  mode: 'mirror' | 'versioned'
  keepChanged: boolean
  keepChangedDays: number
  keepLastOn: boolean
  keepLast: number
  keepDaysOn: boolean
  keepDays: number
  scheduled: boolean
  frequency: Frequency
  /** 0 is Monday. */
  weekday: number
  /** `HH:MM`, the NAS's time. */
  time: string
  onPlug: boolean
  excludes: string[]
  hidden: boolean
  bandwidth: Bandwidth
  bwlimitKib: number
  notifySuccess: boolean
}

export function emptyServer(transport: Transport = 'ssh'): ServerForm {
  return {
    transport,
    host: '',
    port: transport === 'ssh' ? SSH_PORT : DAEMON_PORT,
    user: '',
    module: '',
    password: '',
    hostKey: null,
    fingerprint: null,
  }
}

export function emptyForm(kind: TaskKind = 'local'): TaskForm {
  return {
    name: '',
    kind,
    source: '',
    targetKind: 'share',
    target: '',
    drive: '',
    drivePath: '',
    server: emptyServer(),
    serverPath: '',
    mode: 'versioned',
    keepChanged: true,
    keepChangedDays: 30,
    keepLastOn: true,
    keepLast: 30,
    keepDaysOn: false,
    keepDays: 90,
    scheduled: true,
    frequency: 'daily',
    weekday: 0,
    time: '03:00',
    onPlug: false,
    excludes: [...DEFAULT_EXCLUDES],
    hidden: true,
    bandwidth: 'default',
    bwlimitKib: 10_240,
    notifySuccess: false,
  }
}

/** A task's kind changed: what fits it stays. */
export function withKind(form: TaskForm, kind: TaskKind): TaskForm {
  const next = { ...form, kind }
  if (kind === 'push') next.onPlug = false
  return next
}

/** Whether `pattern` may be given to rsync: one line, none of its own prefixes. */
export function isPattern(pattern: string): boolean {
  return (
    pattern.length > 0 &&
    new TextEncoder().encode(pattern).length <= 255 &&
    pattern.trim() === pattern &&
    !/\p{Cc}/u.test(pattern) &&
    !pattern.startsWith('+ ') &&
    !pattern.startsWith('- ') &&
    !pattern.startsWith('!')
  )
}

/** The list with `pattern` added, once. */
export function addPattern(list: string[], pattern: string): string[] {
  const clean = pattern.trim()
  return !clean || list.includes(clean) ? list : [...list, clean]
}

export function isHost(host: string): boolean {
  return /^[A-Za-z0-9:][A-Za-z0-9.:-]{0,252}$/.test(host)
}

export function isWord(word: string): boolean {
  return /^[A-Za-z0-9_][A-Za-z0-9._-]{0,63}$/.test(word)
}

/** Names separated by `/`, none `.` or `..`; on a server also from `/`. */
export function isFolderPath(path: string, server = false): boolean {
  const relative = server && path.startsWith('/') ? path.slice(1) : path
  if (server && path.startsWith('-')) return false
  if (!relative) return true
  return relative
    .split('/')
    .every((name) => name !== '' && name !== '.' && name !== '..' && !/\p{Cc}/u.test(name))
}

/** `HH:MM` as hour and minute. */
export function parseTime(time: string): { hour: number; minute: number } | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(time.trim())
  if (!match) return null
  const [hour, minute] = [Number(match[1]), Number(match[2])]
  return hour <= 23 && minute <= 59 ? { hour, minute } : null
}

export function timeOf(hour: number, minute: number): string {
  return `${String(hour).padStart(2, '0')}:${String(minute).padStart(2, '0')}`
}

/** `share/folder` as its share and folder. */
export function splitPlace(value: string): { share: string; path: string } {
  const [share = '', ...rest] = value.split('/')
  return { share, path: rest.join('/') }
}

/** Whether one folder of a share lies in the other, or is it. */
export function nested(a: string, b: string): boolean {
  const pa = splitPlace(a)
  const pb = splitPlace(b)
  if (!pa.share || pa.share !== pb.share) return false
  const within = (inner: string, outer: string) =>
    !outer || inner === outer || inner.startsWith(`${outer}/`)
  return within(pa.path, pb.path) || within(pb.path, pa.path)
}

export type Problem =
  | 'name'
  | 'nameTaken'
  | 'source'
  | 'target'
  | 'nested'
  | 'drive'
  | 'host'
  | 'port'
  | 'user'
  | 'module'
  | 'hostKey'
  | 'serverPath'
  | 'retention'
  | 'time'
  | 'excludes'
  | 'bandwidth'

/** What keeps each step of the wizard from going on. */
export const STEP_PROBLEMS: Problem[][] = [
  [],
  [
    'name',
    'nameTaken',
    'source',
    'target',
    'nested',
    'drive',
    'host',
    'port',
    'user',
    'module',
    'hostKey',
    'serverPath',
  ],
  ['retention', 'time', 'excludes', 'bandwidth'],
  [],
]

const between = (value: number, low: number, high: number) =>
  Number.isInteger(value) && value >= low && value <= high

/** What is wrong with `form`; `taken`: the other tasks' names. */
export function problems(form: TaskForm, taken: string[]): Problem[] {
  const found: Problem[] = []
  const name = form.name.trim()
  if (!name || [...name].length > 64) found.push('name')
  else if (taken.some((other) => other.toLowerCase() === name.toLowerCase()))
    found.push('nameTaken')
  const fromNas = form.kind !== 'pull'
  const intoNas = form.kind !== 'push'
  if (fromNas && !form.source) found.push('source')
  if (intoNas && form.targetKind === 'share' && !form.target) found.push('target')
  if (intoNas && form.targetKind === 'drive' && (!form.drive || !isFolderPath(form.drivePath)))
    found.push('drive')
  if (
    form.kind === 'local' &&
    form.targetKind === 'share' &&
    form.source &&
    form.target &&
    nested(form.source, form.target)
  )
    found.push('nested')
  if (form.kind !== 'local') {
    const server = form.server
    if (!isHost(server.host)) found.push('host')
    if (!between(server.port, 1, 65535)) found.push('port')
    if (!isWord(server.user)) found.push('user')
    if (server.transport === 'daemon' && !isWord(server.module)) found.push('module')
    if (server.transport === 'ssh' && !server.hostKey) found.push('hostKey')
    if (!isFolderPath(form.serverPath, true)) found.push('serverPath')
  }
  if (form.mode === 'versioned') {
    if (form.keepLastOn && !between(form.keepLast, 1, MAX_KEEP_VERSIONS)) found.push('retention')
    else if (form.keepDaysOn && !between(form.keepDays, 1, MAX_KEEP_DAYS)) found.push('retention')
  } else if (form.keepChanged && !between(form.keepChangedDays, 1, MAX_KEEP_DAYS)) {
    found.push('retention')
  }
  if (form.scheduled && !parseTime(form.time)) found.push('time')
  if (form.excludes.length > MAX_EXCLUDES || !form.excludes.every(isPattern)) found.push('excludes')
  if (form.bandwidth === 'custom' && !between(form.bwlimitKib, 1, MAX_BWLIMIT_KIB))
    found.push('bandwidth')
  return found
}

function server(form: TaskForm): BackupServer {
  const s = form.server
  return s.transport === 'daemon'
    ? {
        transport: 'daemon',
        host: s.host.trim(),
        port: s.port,
        user: s.user.trim(),
        module: s.module.trim(),
      }
    : {
        transport: 'ssh',
        host: s.host.trim(),
        port: s.port,
        user: s.user.trim(),
        host_key: s.hostKey,
        fingerprint: s.fingerprint,
      }
}

function sharePlace(value: string, shareIds: Map<string, number>): BackupPlace {
  const { share, path } = splitPlace(value)
  return { kind: 'share', share_id: shareIds.get(share) ?? 0, path }
}

/** The request `form` makes; `shareIds`: the shares by name. */
export function request(form: TaskForm, shareIds: Map<string, number>): BackupTaskRequest {
  const time = parseTime(form.time) ?? { hour: 0, minute: 0 }
  const schedule: BackupSchedule | null = form.scheduled
    ? {
        frequency: form.frequency,
        weekday: form.frequency === 'weekly' ? form.weekday : null,
        hour: form.frequency === 'hourly' ? 0 : time.hour,
        minute: time.minute,
      }
    : null
  const serverPlace: BackupPlace = { kind: 'server', path: form.serverPath.trim() }
  const target: BackupPlace =
    form.kind === 'push'
      ? serverPlace
      : form.targetKind === 'drive'
        ? { kind: 'drive', drive: form.drive, path: form.drivePath }
        : sharePlace(form.target, shareIds)
  return {
    name: form.name.trim(),
    source: form.kind === 'pull' ? serverPlace : sharePlace(form.source, shareIds),
    target,
    server: form.kind === 'local' ? null : server(form),
    password:
      form.kind !== 'local' && form.server.transport === 'daemon' && form.server.password
        ? form.server.password
        : null,
    mode:
      form.mode === 'mirror'
        ? { kind: 'mirror', keep_changed_days: form.keepChanged ? form.keepChangedDays : null }
        : {
            kind: 'versioned',
            keep_last: form.keepLastOn ? form.keepLast : null,
            keep_days: form.keepDaysOn ? form.keepDays : null,
          },
    schedule,
    on_plug: form.kind !== 'push' && form.targetKind === 'drive' && form.onPlug,
    excludes: form.excludes,
    hidden: form.hidden,
    bwlimit_kib:
      form.bandwidth === 'default' ? null : form.bandwidth === 'none' ? 0 : form.bwlimitKib,
    notify_success: form.notifySuccess,
  }
}

/** A share's folder as the folder picker names it. */
function pickerValue(place: BackupPlace, shareNames: Map<number, string>): string {
  if (place.kind !== 'share') return ''
  const share = shareNames.get(place.share_id) ?? ''
  return place.path ? `${share}/${place.path}` : share
}

/** The form of a task kept, to change it; `shareNames`: the shares by id. */
export function formOf(task: BackupTask, shareNames: Map<number, string>): TaskForm {
  const form = emptyForm(task.kind)
  form.name = task.name
  form.source = pickerValue(task.source, shareNames)
  if (task.target.kind === 'drive') {
    form.targetKind = 'drive'
    form.drive = task.target.drive
    form.drivePath = task.target.path
  } else if (task.target.kind === 'share') {
    form.target = pickerValue(task.target, shareNames)
  }
  const serverPlace = [task.source, task.target].find((place) => place.kind === 'server')
  if (serverPlace?.kind === 'server') form.serverPath = serverPlace.path
  if (task.server) {
    form.server = {
      transport: task.server.transport,
      host: task.server.host,
      port: task.server.port,
      user: task.server.user,
      module: task.server.module ?? '',
      password: '',
      hostKey: task.server.host_key ?? null,
      fingerprint: task.server.fingerprint ?? null,
    }
  }
  if (task.mode.kind === 'mirror') {
    form.mode = 'mirror'
    form.keepChanged = task.mode.keep_changed_days != null
    form.keepChangedDays = task.mode.keep_changed_days ?? 30
  } else {
    form.mode = 'versioned'
    form.keepLastOn = task.mode.keep_last != null
    form.keepLast = task.mode.keep_last ?? 30
    form.keepDaysOn = task.mode.keep_days != null
    form.keepDays = task.mode.keep_days ?? 90
  }
  form.scheduled = task.schedule != null
  if (task.schedule) {
    form.frequency = task.schedule.frequency
    form.weekday = task.schedule.weekday ?? 0
    form.time = timeOf(task.schedule.hour ?? 0, task.schedule.minute)
  }
  form.onPlug = task.on_plug
  form.excludes = [...task.excludes]
  form.hidden = task.hidden
  form.bandwidth = task.bwlimit_kib == null ? 'default' : task.bwlimit_kib === 0 ? 'none' : 'custom'
  form.bwlimitKib = task.bwlimit_kib || 10_240
  form.notifySuccess = task.notify_success
  return form
}

/** How a task stands, in a word. */
export type TaskStatus = 'running' | 'succeeded' | 'partial' | 'failed' | 'never' | 'unreachable'

export function statusOf(task: BackupTask): TaskStatus {
  if (task.running) return 'running'
  if (!task.reachable) return 'unreachable'
  const outcome = task.last_run?.outcome
  if (!outcome) return 'never'
  if (outcome === 'succeeded') return 'succeeded'
  if (outcome === 'partial') return 'partial'
  return outcome === 'running' ? 'running' : 'failed'
}

/** The tasks at a glance: whether all is well, the last run and the next. */
export function overview(tasks: BackupTask[]): {
  state: 'empty' | 'fine' | 'running' | 'attention'
  running: number
  attention: BackupTask[]
  last: { task: BackupTask; run: BackupRun } | null
  next: { task: BackupTask; at: number } | null
} {
  const statuses = tasks.map((task) => [task, statusOf(task)] as const)
  const attention = statuses
    .filter(([, status]) => status === 'failed' || status === 'partial' || status === 'unreachable')
    .map(([task]) => task)
  const running = statuses.filter(([, status]) => status === 'running').length
  let last: { task: BackupTask; run: BackupRun } | null = null
  let next: { task: BackupTask; at: number } | null = null
  for (const task of tasks) {
    const run = task.last_run
    if (run && (!last || run.started_at > last.run.started_at)) last = { task, run }
    if (task.next_at != null && (!next || task.next_at < next.at)) next = { task, at: task.next_at }
  }
  const state = !tasks.length
    ? 'empty'
    : attention.length
      ? 'attention'
      : running
        ? 'running'
        : 'fine'
  return { state, running, attention, last, next }
}

/** A run's length in seconds; none while it runs. */
export function duration(run: BackupRun): number | null {
  return run.finished_at == null ? null : Math.max(0, run.finished_at - run.started_at)
}

/** The path of an entry in a folder of a backup. */
export function joinPath(folder: string, name: string): string {
  return folder ? `${folder}/${name}` : name
}

/** The folder above; the top's is itself. */
export function parentPath(folder: string): string {
  const at = folder.lastIndexOf('/')
  return at < 0 ? '' : folder.slice(0, at)
}

/** Paths chosen to restore, without those inside another chosen one. */
export function restorePaths(chosen: string[]): string[] {
  const sorted = [...new Set(chosen)].sort()
  return sorted.filter(
    (path) => !sorted.some((other) => other !== path && path.startsWith(`${other}/`)),
  )
}

/** The folder a task's backup goes into, from its name, as the NAS names it. */
export function folderName(name: string): string {
  const replaced = [...name]
    .map((c) => (c === '/' || c === '\\' || /\p{Cc}/u.test(c) ? '_' : c))
    .join('')
  const trimmed = replaced.replace(/^[.\s]+|[.\s]+$/gu, '')
  return trimmed || 'backup'
}
