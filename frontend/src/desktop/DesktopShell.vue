<script setup lang="ts">
import { LogOut } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import { useSessionStore } from '@/features/auth/store'
import { useSystemInfo } from '@/features/system/api'
import { BrandMark } from '@/shared/ui/brand'
import { Button } from '@/shared/ui/button'

const { t } = useI18n()
const router = useRouter()
const session = useSessionStore()
const { data: system } = useSystemInfo()

const accountName = computed(() => session.info?.account.name ?? '')

async function signOut(): Promise<void> {
  await session.signOut()
  await router.push({ name: 'login' })
}
</script>

<!-- Placeholder until the desktop with windows arrives (milestone M1). -->
<template>
  <div class="wallpaper flex h-dvh flex-col">
    <main class="flex flex-1 items-center justify-center p-6">
      <div
        class="flex max-w-md flex-col items-center gap-3 rounded-xl border bg-card/80 p-8 text-center shadow-sm backdrop-blur"
      >
        <BrandMark class="size-12" />
        <h1 class="text-xl font-semibold">{{ t('desktop.welcome', { name: accountName }) }}</h1>
        <p v-if="system" class="text-sm text-muted-foreground">
          {{ t('desktop.system', { os: system.os_name ?? 'Linux', host: system.hostname }) }}
        </p>
        <p class="text-sm text-muted-foreground">{{ t('desktop.placeholder') }}</p>
      </div>
    </main>
    <footer class="flex h-12 items-center gap-3 border-t bg-background/80 px-3 backdrop-blur">
      <BrandMark class="size-6" />
      <span class="text-sm font-medium">{{ t('app.name') }}</span>
      <span class="flex-1" />
      <span class="text-sm text-muted-foreground">{{ accountName }}</span>
      <Button variant="ghost" size="sm" @click="signOut">
        <LogOut />
        {{ t('desktop.signOut') }}
      </Button>
    </footer>
  </div>
</template>
