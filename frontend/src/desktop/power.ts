import { defineStore } from 'pinia'
import { ref } from 'vue'

import { isServerReachable, requestPowerAction } from '@/features/system/api'
import { live } from '@/shared/api/live'
import type { PowerAction } from '@/shared/api/types'

const POLL_MS = 3000
/** Reload anyway after this long, in case the outage was too short to notice. */
const GIVE_UP_WAITING_FOR_DOWNTIME_MS = 90_000

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

/** Restart and shutdown, and what the page shows meanwhile. */
export const usePower = defineStore('power', () => {
  const phase = ref<'running' | 'restarting' | 'off'>('running')

  /** Throws if the server refused; the caller explains why. */
  async function request(action: PowerAction): Promise<void> {
    await requestPowerAction(action)
    live.disconnect()
    phase.value = action === 'reboot' ? 'restarting' : 'off'
    if (action === 'reboot') void reloadWhenBack()
  }

  async function reloadWhenBack(): Promise<void> {
    const started = Date.now()
    let wentDown = false
    for (;;) {
      await sleep(POLL_MS)
      if (!(await isServerReachable())) wentDown = true
      else if (wentDown || Date.now() - started > GIVE_UP_WAITING_FOR_DOWNTIME_MS) {
        window.location.reload()
        return
      }
    }
  }

  return { phase, request }
})
