import { ref } from 'vue'

import { api, unwrap } from '@/shared/api/client'

/**
 * The machine's name as the pages before the sign-in show it. The NAS tells
 * it only to a client on its own network, so it stays `null` for anyone
 * else, and until it has been asked for.
 */
export const deviceName = ref<string | null>(null)

/** Asks for the name; a NAS that does not answer has none. */
export async function loadDeviceName(): Promise<void> {
  try {
    deviceName.value = (await unwrap(api.GET('/api/v1/system/name'))).name ?? null
  } catch {
    deviceName.value = null
  }
}
