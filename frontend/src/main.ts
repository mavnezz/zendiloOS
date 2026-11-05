import '@/shared/styles/global.css'

import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { createPinia } from 'pinia'
import { createApp } from 'vue'

import App from '@/app/App.vue'
import { installNavigationGuard } from '@/app/navigation'
import { createAppRouter } from '@/app/router'
import { useSessionStore } from '@/features/auth/store'
import { ApiProblem, configureApi } from '@/shared/api/client'
import { i18n } from '@/shared/i18n'
import { applyTheme, followSystemTheme } from '@/shared/lib/theme'

applyTheme()
followSystemTheme()

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

configureApi({
  csrfToken: () => session.info?.csrf_token,
  onUnauthenticated: () => {
    session.clear()
    queryClient.clear()
    void router.push({ name: 'login' })
  },
})
installNavigationGuard(router, session)

app.use(router)
app.mount('#app')
