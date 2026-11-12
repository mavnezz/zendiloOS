// Snapshots and backups of VMs as their dialogs see them: what a rollback
// destroys, how consistent a snapshot is, where a backup goes, and the
// job a VM's backup or restore runs in. The server checks the same rules.

import type {
  Job,
  VmBackupTarget,
  VmConsistency,
  VmSnapshot,
  VmSnapshotFrequency,
  VmSnapshotSchedule,
} from '@/shared/api/types'

export const FREQUENCIES: VmSnapshotFrequency[] = ['hourly', 'daily', 'weekly', 'monthly']
export const KEEP_MAX = 100
export const SNAPSHOT_NAME_MAX = 64
/** Where a backup goes in a share or on a drive unless another folder is given. */
export const DEFAULT_FOLDER = 'vm-backups'
/** The jobs a VM's backup or restore runs in. */
export const WORK_KINDS = ['vms.backup', 'vms.restore']

/** The labels of the snapshots taken after `label`, which a rollback to it destroys. */
export function newerThan(snapshots: readonly VmSnapshot[], label: string): string[] {
  const target = snapshots.find((snapshot) => snapshot.label === label)
  if (!target) return []
  return snapshots
    .filter((snapshot) => snapshot.taken_at > target.taken_at)
    .map((snapshot) => snapshot.label)
}

/** How a snapshot's consistency is shown. */
export function consistencyVariant(consistency: VmConsistency): 'success' | 'info' | 'warning' {
  return consistency === 'off' ? 'success' : consistency === 'frozen' ? 'info' : 'warning'
}

/** Whether a snapshot's name is one the server takes. */
export function snapshotNameValid(name: string): boolean {
  const trimmed = name.trim()
  return [...trimmed].length <= SNAPSHOT_NAME_MAX && !/[\u0000-\u001f\u007f]/.test(trimmed)
}

/** The keep of each frequency, `null` where there is no schedule. */
export type ScheduleDraft = Record<VmSnapshotFrequency, number | null>

export function scheduleDraft(schedules: readonly VmSnapshotSchedule[]): ScheduleDraft {
  const draft: ScheduleDraft = { hourly: null, daily: null, weekly: null, monthly: null }
  for (const schedule of schedules) draft[schedule.frequency] = schedule.keep
  return draft
}

/** The schedules a draft asks for; `null` while a keep is not 1 to 100. */
export function scheduleList(draft: ScheduleDraft): VmSnapshotSchedule[] | null {
  const list: VmSnapshotSchedule[] = []
  for (const frequency of FREQUENCIES) {
    const keep = draft[frequency]
    if (keep === null) continue
    if (!Number.isInteger(keep) || keep < 1 || keep > KEEP_MAX) return null
    list.push({ frequency, keep })
  }
  return list
}

/** Whether a folder in a share or on a drive is one: names without `.`, `..` or empty parts. */
export function folderValid(folder: string): boolean {
  const path = folder.trim().replace(/^\/+|\/+$/g, '')
  if (path === '') return true
  return (
    path.length <= 1024 &&
    path
      .split('/')
      .every(
        (name) =>
          name !== '' &&
          name !== '.' &&
          name !== '..' &&
          name.length <= 255 &&
          !/[\u0000-\u001f\u007f]/.test(name),
      )
  )
}

/**
 * Where a backup goes: a folder picked in a share (`share/folder/…`), or a
 * folder on a drive; `null` while that is none.
 */
export function backupTarget(
  kind: 'share' | 'drive',
  path: string,
  shares: readonly { id: number; name: string }[],
  drive: string,
): { target: VmBackupTarget; folder: string } | null {
  if (kind === 'drive') {
    if (!drive || !folderValid(path)) return null
    return { target: { kind: 'drive', drive }, folder: path.trim().replace(/^\/+|\/+$/g, '') }
  }
  const [share = '', ...rest] = path.split('/').filter(Boolean)
  const found = shares.find((candidate) => candidate.name === share)
  const folder = rest.join('/')
  if (!found || !folderValid(folder)) return null
  return { target: { kind: 'share', share_id: found.id }, folder }
}

/** The backup or restore of the VM named `name` that runs, if one does. */
export function workOf(jobs: readonly Job[], name: string): Job | undefined {
  return jobs.find(
    (job) => job.state === 'running' && WORK_KINDS.includes(job.kind) && job.target === name,
  )
}
