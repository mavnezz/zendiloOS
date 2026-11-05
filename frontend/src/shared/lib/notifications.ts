import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

export type NotificationKind = 'info' | 'success' | 'warning' | 'error'

export interface AppNotification {
  id: number
  kind: NotificationKind
  title: string
  body?: string
  /** Milliseconds since the epoch. */
  at: number
  read: boolean
}

const KEEP = 50
const TOAST_MS = 5000

/** Messages for the notification center; new ones also appear as toasts. */
export const useNotifications = defineStore('notifications', () => {
  const items = ref<AppNotification[]>([])
  const toastIds = ref<number[]>([])
  let nextId = 1

  const unread = computed(() => items.value.filter((item) => !item.read).length)
  const toasts = computed(() =>
    toastIds.value.flatMap((id) => items.value.filter((item) => item.id === id)),
  )

  function notify(kind: NotificationKind, title: string, body?: string): void {
    const id = nextId++
    items.value = [{ id, kind, title, body, at: Date.now(), read: false }, ...items.value].slice(
      0,
      KEEP,
    )
    toastIds.value.push(id)
    setTimeout(() => dismissToast(id), TOAST_MS)
  }

  function dismissToast(id: number): void {
    toastIds.value = toastIds.value.filter((toast) => toast !== id)
  }

  function markAllRead(): void {
    items.value = items.value.map((item) => ({ ...item, read: true }))
  }

  function clear(): void {
    items.value = []
    toastIds.value = []
  }

  return { items, unread, toasts, notify, dismissToast, markAllRead, clear }
})
