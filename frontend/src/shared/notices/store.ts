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
  // A disk the enclosure places is named with its slot, as the pages name it.
  if (notification.code === 'disk_health' && params.slot) {
    params.disk = `${params.disk} (${t('storage.enclosure.slot', { slot: params.slot, group: params.group })})`
  }
  if (notification.code === 'firewall_blocked') {
    const service = `${key}.services.${params.service}`
    if (params.project) params.service = t(`${key}.project`, { name: params.project })
    else if (te(service)) params.service = t(service)
  }
  if (!te(`${key}.title`)) return { title: notification.code }
  const count = Number(params.errors ?? 0)
  let body = `${key}.body`
  if (notification.code === 'updates_available' && !params.version) body = `${key}.bodyDebian`
  if (notification.code === 'updates_installed') {
    if (params.zfs_ready === 'false') body = `${key}.bodyZfs`
    else if (params.restart === 'true') body = `${key}.bodyRestart`
  }
  return {
    title: t(`${key}.title`, params),
    body: te(body) ? t(body, params, count) : undefined,
  }
}

/**
 * Notifications from the server (degraded pools, failing disks, …) in the
 * notification center. Live ones also appear as toasts. What the account
 * has read or cleared away is kept on the server, so every browser of the
 * account shows the same.
 */
export const useServerNotices = defineStore('server-notices', () => {
  let unsubscribe: (() => void) | null = null
  let offReconnect: (() => void) | null = null
  const shown = new Set<number>()
  let newest = 0

  function show(
    notification: ServerNotification,
    options: { toast: boolean; read: boolean },
  ): void {
    // A live one may also be in the list fetched at the same time.
    if (shown.has(notification.id)) return
    shown.add(notification.id)
    newest = Math.max(newest, notification.id)
    const { title, body } = wording(notification)
    useNotifications().notify(notification.level, title, body, {
      at: notification.at * 1000,
      ...options,
    })
  }

  /** The recent ones from the server; those shown already stay as they are. */
  async function load(): Promise<void> {
    try {
      const list = await unwrap(
        api.GET('/api/v1/notifications', { params: { query: { limit: 50 } } }),
      )
      for (const notification of list.notifications.reverse()) {
        show(notification, { toast: false, read: notification.id <= list.read_up_to })
      }
    } catch {
      // Live notices still arrive.
    }
  }

  /** Shows the recent ones and follows new ones, also across reconnects. */
  async function start(): Promise<void> {
    if (unsubscribe) return
    unsubscribe = live.subscribe('notifications', (notification) =>
      show(notification, { toast: true, read: false }),
    )
    // What came while the connection was down was not delivered live.
    offReconnect = live.onReconnect(() => void load())
    await load()
  }

  /** Everything shown so far counts as read, in every browser. */
  async function markRead(): Promise<void> {
    if (newest === 0) return
    try {
      await unwrap(api.POST('/api/v1/notifications/read', { body: { up_to: newest } }))
    } catch {
      // They show as unread again next time.
    }
  }

  /** Everything shown so far is cleared away, in every browser. */
  async function clear(): Promise<void> {
    if (newest === 0) return
    try {
      await unwrap(api.POST('/api/v1/notifications/clear', { body: { up_to: newest } }))
    } catch {
      // They come back next time.
    }
  }

  function stop(): void {
    unsubscribe?.()
    unsubscribe = null
    offReconnect?.()
    offReconnect = null
    shown.clear()
    newest = 0
  }

  return { start, markRead, clear, stop }
})
