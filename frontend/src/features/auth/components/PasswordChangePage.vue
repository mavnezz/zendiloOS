<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import { Button } from '@/shared/ui/button'

import { fetchSession } from '../api'
import { useSessionStore } from '../store'
import AuthFrame from './AuthFrame.vue'
import ChangePasswordForm from './ChangePasswordForm.vue'

const { t } = useI18n()
const router = useRouter()
const session = useSessionStore()

async function changed(): Promise<void> {
  const info = await fetchSession()
  if (info) session.start(info)
  else session.clear()
  await router.push({ name: 'desktop' })
}

async function signOut(): Promise<void> {
  await session.signOut()
  await router.push({ name: 'login' })
}
</script>

<!-- An administrator set this account's password; it must choose its own. -->
<template>
  <AuthFrame>
    <div class="grid gap-6 rounded-xl border bg-card p-6 shadow-sm">
      <div class="grid gap-1">
        <h1 class="text-lg font-semibold">{{ t('passwordChange.title') }}</h1>
        <p class="text-sm text-muted-foreground">
          {{ t('passwordChange.intro', { name: session.info?.account.name }) }}
        </p>
      </div>
      <ChangePasswordForm @changed="changed" />
      <Button variant="link" class="justify-self-start px-0" @click="signOut">
        {{ t('desktop.signOut') }}
      </Button>
    </div>
  </AuthFrame>
</template>
