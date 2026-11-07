import type { QueryClient } from '@tanstack/vue-query'

import { useNotifications } from '@/shared/lib/notifications'
import { usePreferences } from '@/shared/preferences/store'

/**
 * Forgets what this browser holds of an account that signed out or whose
 * session ended, so that the next account signing in here sees nothing of
 * it: cached answers (shares, favourites, …), its preferences and its
 * notifications. The desktop forgets its windows itself.
 */
export function forgetAccount(queryClient: QueryClient): void {
  queryClient.clear()
  usePreferences().reset()
  useNotifications().clear()
}
