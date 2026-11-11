import type {
  BackupMode,
  BackupPlace,
  BackupSchedule,
  BackupServer,
  BackupTask,
} from '@/shared/api/types'

import { timeOf } from './backup'

/** vue-i18n's `t`, as far as these words need it. */
export type Translate = (key: string, values?: Record<string, unknown> | number) => string

/** `user@host:port`, as a server is named. */
export function serverName(server: BackupServer): string {
  const standard = server.transport === 'ssh' ? 22 : 873
  const host = server.host.includes(':') ? `[${server.host}]` : server.host
  const port = server.port === standard ? '' : `:${server.port}`
  const module = server.transport === 'daemon' && server.module ? `::${server.module}` : ''
  return `${server.user}@${host}${port}${module}`
}

/** Where a task reads or writes, for a person: the share, drive or server and the folder. */
export function placeText(
  t: Translate,
  place: BackupPlace,
  server: BackupServer | null | undefined,
  shareNames: Map<number, string>,
  driveLabels: Map<string, string> = new Map(),
): string {
  switch (place.kind) {
    case 'share': {
      const share = shareNames.get(place.share_id) ?? `#${place.share_id}`
      return place.path
        ? t('backup.place.share', { share, path: place.path })
        : t('backup.place.shareRoot', { share })
    }
    case 'drive': {
      const drive = driveLabels.get(place.drive) ?? place.drive
      return place.path
        ? t('backup.place.drive', { drive, path: place.path })
        : t('backup.place.driveRoot', { drive })
    }
    case 'server': {
      const name = server ? serverName(server) : '?'
      return place.path
        ? t('backup.place.server', { server: name, path: place.path })
        : t('backup.place.serverRoot', { server: name })
    }
  }
}

/** When a task backs up, in words. */
export function scheduleText(
  t: Translate,
  schedule: BackupSchedule | null | undefined,
  onPlug: boolean,
): string {
  if (!schedule) return onPlug ? t('backup.schedule.onPlug') : t('backup.schedule.off')
  const time = timeOf(schedule.hour ?? 0, schedule.minute)
  const minute = String(schedule.minute).padStart(2, '0')
  const weekday = t(`backup.weekdays.${schedule.weekday ?? 0}`)
  const text =
    schedule.frequency === 'hourly'
      ? t('backup.schedule.hourly', { minute })
      : schedule.frequency === 'daily'
        ? t('backup.schedule.daily', { time })
        : t('backup.schedule.weekly', { weekday, time })
  return onPlug ? t('backup.schedule.withPlug', { schedule: text }) : text
}

/** What a task keeps, in words. */
export function retentionText(t: Translate, mode: BackupMode): string {
  if (mode.kind === 'mirror') {
    return mode.keep_changed_days != null
      ? t('backup.wizard.retention.changed', { days: mode.keep_changed_days })
      : t('backup.wizard.retention.notChanged')
  }
  const parts: string[] = []
  if (mode.keep_last != null)
    parts.push(t('backup.wizard.retention.last', { count: mode.keep_last }))
  if (mode.keep_days != null)
    parts.push(t('backup.wizard.retention.days', { count: mode.keep_days }))
  return parts.length ? parts.join(', ') : t('backup.wizard.retention.all')
}

/** A change of rsync's transfer list as a word: new, changed, deleted, a folder. */
export function changeKind(change: string): 'new' | 'changed' | 'deleted' | 'folder' | 'other' {
  if (change.startsWith('*deleting')) return 'deleted'
  if (change[1] === 'd') return 'folder'
  if (change.slice(2).startsWith('+++++++++')) return 'new'
  if (change.startsWith('>') || change.startsWith('<') || change.startsWith('c')) return 'changed'
  return 'other'
}

/** A task's own server, if it has one. */
export function serverOf(task: BackupTask): BackupServer | null {
  return task.server ?? null
}
