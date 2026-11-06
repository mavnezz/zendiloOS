import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { ApiProblem } from '@/shared/api/client'
import type { InstalledApp } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'

import { useInstallActions } from '../api'
import { appText } from '../texts'

/**
 * Updating installed apps, one or all. A version that asks for more than
 * the installed one is held back until confirmed: `asking` names it.
 */
export function useAppUpdates() {
  const { t, locale } = useI18n()
  const actions = useInstallActions()
  const notifications = useNotifications()
  const busy = ref<string | null>(null)
  const asking = ref<InstalledApp | null>(null)

  /** Starts the update; `false` if it waits for confirmation or failed. */
  async function update(app: InstalledApp, confirm = false): Promise<boolean> {
    const name = appText(app, 'name', locale.value)
    busy.value = app.id
    try {
      await actions.update(app.id, confirm)
      notifications.notify('info', t('appStore.installed.updating', { name }))
      return true
    } catch (problem) {
      if (problem instanceof ApiProblem && problem.code === 'permissions_not_confirmed') {
        asking.value = app
      } else {
        notifications.notify('error', name, problemMessage(problem))
      }
      return false
    } finally {
      busy.value = null
    }
  }

  /** Every app with a newer version, one after the other. */
  async function updateAll(apps: InstalledApp[]): Promise<void> {
    for (const app of apps) {
      if (asking.value) break
      await update(app)
    }
  }

  return { busy, asking, update, updateAll }
}
