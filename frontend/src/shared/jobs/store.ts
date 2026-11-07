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
    } else {
      const code = job.error_code
      const known = code && i18n.global.te(`jobs.errors.${code}`)
      const reason = code
        ? known
          ? t(`jobs.errors.${code}`)
          : t('jobs.failedBecause', { code })
        : ''
      notifications.notify(
        'error',
        jobTitle(job),
        `${t(`jobs.state.${job.state}`)} ${reason}`.trim(),
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
