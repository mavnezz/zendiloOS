import type { BackupPlanView, DockerVolume, Job, PlanFrequency } from '@/shared/api/types'

type Translate = (key: string, values?: Record<string, unknown>) => string

/** The jobs that work on a volume. */
const VOLUME_JOBS = ['containers.volume_backup', 'containers.volume_restore']

/** The job that records the backups found in a folder. */
export const IMPORT_JOB = 'containers.volume_import'

/** An hour of the day as the plans show it: `03:00`. */
export function hourText(hour: number): string {
  return `${String(hour).padStart(2, '0')}:00`
}

/** A plan's schedule in words: "Daily at 03:00", "Sundays at 04:00". */
export function scheduleText(
  plan: Pick<BackupPlanView, 'frequency' | 'weekday' | 'day' | 'hour'>,
  t: Translate,
): string {
  const at = hourText(plan.hour)
  switch (plan.frequency) {
    case 'weekly':
      return t('docker.volumes.schedule.weekly', {
        day: t(`docker.volumes.weekdays.${plan.weekday ?? 0}`),
        at,
      })
    case 'monthly':
      return t('docker.volumes.schedule.monthly', { day: plan.day ?? 1, at })
    default:
      return t('docker.volumes.schedule.daily', { at })
  }
}

/** The backup or restore running for `volume`, if one is. */
export function volumeJob(jobs: readonly Job[], volume: string): Job | undefined {
  return jobs.find(
    (job) => job.state === 'running' && VOLUME_JOBS.includes(job.kind) && job.target === volume,
  )
}

/** Whether any backup, restore or import runs. */
export function backupWorkRuns(jobs: readonly Job[]): boolean {
  return jobs.some(
    (job) => job.state === 'running' && (VOLUME_JOBS.includes(job.kind) || job.kind === IMPORT_JOB),
  )
}

/** Whether a volume matches what was typed into the search: its name or project. */
export function matches(volume: DockerVolume, term: string): boolean {
  const wanted = term.trim().toLowerCase()
  return (
    !wanted ||
    volume.name.toLowerCase().includes(wanted) ||
    (volume.project ?? '').toLowerCase().includes(wanted)
  )
}

/** The backup groups whose volume or archives match what was typed into the search. */
export function groupMatches(
  group: { volume: string; backups: readonly { archive?: string | null }[] },
  term: string,
): boolean {
  const wanted = term.trim().toLowerCase()
  return (
    !wanted ||
    group.volume.toLowerCase().includes(wanted) ||
    group.backups.some((backup) => (backup.archive ?? '').toLowerCase().includes(wanted))
  )
}

/** A folder picked in the folder picker, `share/folder/…`, as a plan names it. */
export function planTarget(
  path: string,
  shares: readonly { id: number; name: string }[],
): { share_id: number; folder: string } | null {
  const [share = '', ...rest] = path.split('/').filter(Boolean)
  const found = shares.find((candidate) => candidate.name === share)
  return found ? { share_id: found.id, folder: rest.join('/') } : null
}

/** A plan's folder as the folder picker shows it: `share/folder/…`. */
export function pickerPath(
  plan: Pick<BackupPlanView, 'share_id' | 'folder'>,
  shares: readonly { id: number; name: string }[],
): string {
  const share = shares.find((candidate) => candidate.id === plan.share_id)?.name
  if (!share) return ''
  return plan.folder ? `${share}/${plan.folder}` : share
}

/** The night's hours a new plan may start at. */
const NIGHT_HOURS = [1, 2, 3, 4, 5]

/**
 * The night's hour the fewest of `plans` take, so that new plans spread
 * over the night (backups run one at a time, the containers stopped
 * meanwhile); among those as free, always the same one for `volume`.
 */
export function quietHour(volume: string, plans: readonly { hour: number }[]): number {
  const taken = (hour: number) => plans.filter((plan) => plan.hour === hour).length
  const fewest = Math.min(...NIGHT_HOURS.map(taken))
  const free = NIGHT_HOURS.filter((hour) => taken(hour) === fewest)
  let hash = 0
  for (const char of volume) hash = (hash * 31 + char.charCodeAt(0)) >>> 0
  return free[hash % free.length] ?? 3
}

/** What a new plan starts as: nightly at `hour`, seven kept, the containers stopped. */
export function defaultPlan(hour = 3): {
  frequency: PlanFrequency
  weekday: number
  day: number
  hour: number
  keep: number
  stop: boolean
} {
  return { frequency: 'daily', weekday: 6, day: 1, hour, keep: 7, stop: true }
}

/** Who may read a backups' share, in words: accounts, then groups. */
export function readersText(
  readers: { accounts: string[]; groups: string[] },
  t: Translate,
): string {
  return [
    ...readers.accounts,
    ...readers.groups.map((group) => t('docker.volumes.plan.group', { name: group })),
  ].join(', ')
}
