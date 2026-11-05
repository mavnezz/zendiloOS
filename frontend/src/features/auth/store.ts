import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import type { SessionInfo } from '@/shared/api/types'

import { fetchSession, logout } from './api'

/** Who is signed in. The API client reads the CSRF token from here. */
export const useSessionStore = defineStore('session', () => {
  const info = ref<SessionInfo | null>(null)
  let loaded = false

  const signedIn = computed(() => info.value !== null)

  /** Asks the server once whether the browser still has a session. */
  async function ensureLoaded(): Promise<void> {
    if (loaded) return
    info.value = await fetchSession()
    loaded = true
  }

  function start(session: SessionInfo): void {
    info.value = session
    loaded = true
  }

  function clear(): void {
    info.value = null
  }

  async function signOut(): Promise<void> {
    try {
      await logout()
    } finally {
      clear()
    }
  }

  function can(permission: string): boolean {
    return info.value?.account.permissions.includes(permission) ?? false
  }

  return { info, signedIn, ensureLoaded, start, clear, signOut, can }
})
