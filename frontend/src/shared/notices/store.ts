import { defineStore } from 'pinia'

import { api, unwrap } from '@/shared/api/client'
import { live } from '@/shared/api/live'
import type { ServerNotification } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'

/** Title and text of a server notification in the interface language. */
export function wording(notification: ServerNotification): { title: string; body?: string } {
  const t = i18n.global.t
  const te = (key: string) => i18n.global.te(key)
  const key = `notices.${notification.code}`
  const params = { ...notification.params }
  if (notification.code === 'pool_trouble' && params.state) {
    params.state = t(`notices.states.${params.state}`)
  }
  if (notification.code === 'disk_health' && params.health) {
    params.health = t(`storage.health.${params.health === 'failing' ? 'failing' : 'warning'}`)
  }
  if (!te(`${key}.title`)) return { title: notification.code }
  const count = Number(params.errors ?? 0)
  return {
    title: t(`${key}.title`, params),
    body: te(`${key}.body`) ? t(`${key}.body`, params, count) : undefined,
  }
}

const SEEN_KEY = 'zendiloos.notices.seen.'

/**
 * Notifications from the server (degraded pools, failing disks, …) in the
 * notification center. Live ones also appear as toasts; those that came
 * while this account was away are added quietly when it signs in.
 */
export const useServerNotices = defineStore('server-notices', () => {
  let unsubscribe: (() => void) | null = null
  let account = ''

  function seen(): number {
    try {
      return Number(localStorage.getItem(SEEN_KEY + account) ?? 0)
    } catch {
      return 0
    }
  }

  function markSeen(id: number): void {
    try {
      if (id > seen()) localStorage.setItem(SEEN_KEY + account, String(id))
    } catch {
      // Storage unavailable: older notices may show again next time.
    }
  }

  function show(notification: ServerNotification, toast: boolean): void {
    const { title, body } = wording(notification)
    useNotifications().notify(notification.level, title, body, {
      at: notification.at * 1000,
      toast,
    })
    markSeen(notification.id)
  }

  async function start(name: string): Promise<void> {
    if (unsubscribe) return
    account = name
    unsubscribe = live.subscribe('notifications', (notification) => show(notification, true))
    try {
      const recent = await unwrap(
        api.GET('/api/v1/notifications', { params: { query: { limit: 20 } } }),
      )
      const since = seen()
      for (const notification of recent.reverse()) {
        if (notification.id > since) show(notification, false)
      }
    } catch {
      // Live notices still arrive.
    }
  }

  function stop(): void {
    unsubscribe?.()
    unsubscribe = null
  }

  return { start, stop }
})
