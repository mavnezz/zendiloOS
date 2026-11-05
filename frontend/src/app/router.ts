import { createRouter, createWebHistory, type Router } from 'vue-router'

import LoginPage from '@/features/auth/components/LoginPage.vue'
import PasswordChangePage from '@/features/auth/components/PasswordChangePage.vue'
import SetupPage from '@/features/auth/components/SetupPage.vue'

/**
 * Four pages; which one is shown follows from the session (see
 * navigation.ts). Apps open in windows on the desktop, not via URLs.
 */
export function createAppRouter(): Router {
  return createRouter({
    history: createWebHistory(),
    routes: [
      { path: '/setup', name: 'setup', component: SetupPage },
      { path: '/login', name: 'login', component: LoginPage },
      { path: '/password', name: 'password', component: PasswordChangePage },
      { path: '/', name: 'desktop', component: () => import('@/desktop/DesktopShell.vue') },
      { path: '/:pathMatch(.*)*', redirect: '/' },
    ],
  })
}
