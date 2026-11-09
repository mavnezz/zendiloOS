import '@/shared/styles/global.css'

import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { createPinia } from 'pinia'
import { createApp } from 'vue'

import App from '@/app/App.vue'
import { installNavigationGuard } from '@/app/navigation'
import { createAppRouter } from '@/app/router'
import { useSessionStore } from '@/features/auth/store'
import { ApiProblem, configureApi } from '@/shared/api/client'
import { live } from '@/shared/api/live'
import { i18n } from '@/shared/i18n'
import { forgetAccount } from '@/shared/lib/forget'
import { reloadOnStaleBuild } from '@/shared/lib/stale-build'
import { applyTheme, followSystemTheme } from '@/shared/lib/theme'
import { applyWallpaper, storedWallpaper } from '@/shared/lib/wallpapers'

applyTheme()
followSystemTheme()
applyWallpaper(storedWallpaper())

// A tab open across an update of zendiloOS: the parts of its build are
// gone, so it loads the new build, once. The entry script's URL names the
// build (its file name carries a hash of its content).
reloadOnStaleBuild(window, () => ({
  build: import.meta.url,
  storage: (() => {
    try {
      return window.sessionStorage
    } catch {
      return undefined
    }
  })(),
  now: Date.now,
  reload: () => window.location.reload(),
}))

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // Client errors do not go away by asking again.
      retry: (failures, error) =>
        !(error instanceof ApiProblem && error.status >= 400 && error.status < 500) && failures < 2,
    },
  },
})

const app = createApp(App)
const pinia = createPinia()
app.use(pinia).use(i18n).use(VueQueryPlugin, { queryClient })

const router = createAppRouter()
const session = useSessionStore(pinia)

function sessionEnded(): void {
  session.clear()
  forgetAccount(queryClient)
  void router.push({ name: 'login' })
}

configureApi({ csrfToken: () => session.info?.csrf_token, onUnauthenticated: sessionEnded })
live.onSessionEnded = sessionEnded
installNavigationGuard(router, session)

app.use(router)
app.mount('#app')
