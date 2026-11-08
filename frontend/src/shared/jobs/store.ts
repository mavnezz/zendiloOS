import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import { live } from '@/shared/api/live'
import type { Job } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'

/** How the interface names a job, e.g. "Create pool tank". */
export function jobTitle(job: Job): string {
  const key = `jobs.kinds.${job.kind.replaceAll('.', '_')}`
  const name = i18n.global.te(key) ? i18n.global.t(key) : job.kind
  return job.target ? `${name}: ${job.target}` : name
}

/** Moves that left part of their sources: neither a success nor a plain failure. */
const PARTIAL = ['partly_moved', 'copied_not_moved']

/**
 * The detail of a move that left part of its source: how many entries
 * stayed, a space, and one of them (`photos/album/new.txt`).
 */
export function leftBehind(detail?: string | null): { count: number; example: string } {
  const match = /^(\d+) (.*)$/s.exec(detail ?? '')
  return match ? { count: Number(match[1]), example: match[2] ?? '' } : { count: 0, example: '' }
}

/** Why `job` failed, in words where the reason is known. */
function reason(job: Job): string {
  const { t } = i18n.global
  const code = job.error_code
  if (!code) return ''
  if (PARTIAL.includes(code)) {
    const { count, example } = leftBehind(job.error_detail)
    return t(`jobs.errors.${code}`, { count, example }, count)
  }
  if (!i18n.global.te(`jobs.errors.${code}`)) return t('jobs.failedBecause', { code })
  const message = t(`jobs.errors.${code}`)
  // The files agent names what was not possible, such as two names that
  // differ only in case where the target does not tell them apart.
  return code === 'invalid_operation' && job.error_detail
    ? `${message} (${job.error_detail})`
    : message
}

/**
 * Long-running operations, kept current over the live socket. Every
 * browser hears about every job, so a second administrator sees what the
 * first one started.
 */
export const useJobsStore = defineStore('jobs', () => {
  const jobs = ref(new Map<number, Job>())
  let unsubscribe: (() => void) | null = null
  let offReconnect: (() => void) | null = null

  const list = computed(() => [...jobs.value.values()].sort((a, b) => b.id - a.id))
  const running = computed(() => list.value.filter((job) => job.state === 'running'))

  function update(job: Job): void {
    const previous = jobs.value.get(job.id)
    jobs.value.set(job.id, job)
    if (previous && previous.state === 'running' && job.state !== 'running') announce(job)
  }

  function announce(job: Job): void {
    const { t } = i18n.global
    const notifications = useNotifications()
    if (job.state === 'succeeded') {
      notifications.notify('success', jobTitle(job), t('jobs.succeeded'))
    } else if (job.error_code && PARTIAL.includes(job.error_code)) {
      notifications.notify('warning', jobTitle(job), reason(job))
    } else {
      notifications.notify(
        'error',
        jobTitle(job),
        `${t(`jobs.state.${job.state}`)} ${reason(job)}`.trim(),
      )
    }
  }

  /**
   * The recent jobs as the server has them. After a reconnect, a job known
   * as running that ended meanwhile is announced as if its end had come
   * live.
   */
  async function reload(): Promise<void> {
    try {
      const recent = await unwrap(api.GET('/api/v1/jobs', { params: { query: { limit: 50 } } }))
      for (const job of recent) {
        if (jobs.value.has(job.id)) update(job)
        else jobs.value.set(job.id, job)
      }
    } catch {
      // Without the list, live updates still arrive.
    }
  }

  /** Loads recent jobs and follows changes, also across reconnects; idempotent. */
  async function start(): Promise<void> {
    if (unsubscribe) return
    unsubscribe = live.subscribe('jobs', update)
    offReconnect = live.onReconnect(() => void reload())
    await reload()
  }

  function stop(): void {
    unsubscribe?.()
    unsubscribe = null
    offReconnect?.()
    offReconnect = null
    jobs.value.clear()
  }

  /** Waits for `id` to finish; resolves with the final job. */
  function settled(id: number): Promise<Job> {
    return new Promise((resolve) => {
      const check = () => {
        const job = jobs.value.get(id)
        if (job && job.state !== 'running') {
          off()
          resolve(job)
        }
      }
      const off = live.subscribe('jobs', check)
      check()
    })
  }

  return { jobs, list, running, update, start, stop, settled }
})
