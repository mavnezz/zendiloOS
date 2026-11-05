<script setup lang="ts">
import { LogOut, Power, RotateCcw, Settings, UserRound } from 'lucide-vue-next'
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuPortal,
  DropdownMenuRoot,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from 'reka-ui'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { useSessionStore } from '@/features/auth/store'
import type { PowerAction } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'

import { usePower } from './power'

const emit = defineEmits<{ openSettings: []; signOut: [] }>()

const { t } = useI18n()
const session = useSessionStore()
const power = usePower()
const notifications = useNotifications()

const account = computed(() => session.info?.account)
const confirming = ref<PowerAction | null>(null)
const dialogOpen = computed({
  get: () => confirming.value !== null,
  set: (open) => {
    if (!open) confirming.value = null
  },
})

const item =
  'flex cursor-default items-center gap-2 rounded-md px-2 py-1.5 text-sm outline-none select-none data-[highlighted]:bg-accent'

async function confirmed(): Promise<void> {
  const action = confirming.value
  confirming.value = null
  if (!action) return
  try {
    await power.request(action)
  } catch (error) {
    notifications.notify('error', t('desktop.powerFailed'), problemMessage(error))
  }
}
</script>

<template>
  <DropdownMenuRoot>
    <DropdownMenuTrigger
      class="flex h-9 items-center gap-2 rounded-md px-2 text-sm hover:bg-accent data-[state=open]:bg-accent"
      :aria-label="t('desktop.account')"
    >
      <UserRound class="size-4" />
      <span class="hidden max-w-32 truncate sm:inline">{{ account?.name }}</span>
    </DropdownMenuTrigger>
    <DropdownMenuPortal>
      <DropdownMenuContent
        side="top"
        align="end"
        :side-offset="8"
        class="z-[900] min-w-52 rounded-xl border bg-card p-1 shadow-xl"
      >
        <DropdownMenuLabel class="px-2 py-1.5">
          <p class="text-sm font-medium">{{ account?.name }}</p>
          <p v-if="account" class="text-xs text-muted-foreground">
            {{ t(`settings.roles.${account.role}`) }}
          </p>
        </DropdownMenuLabel>
        <DropdownMenuSeparator class="my-1 h-px bg-border" />
        <DropdownMenuItem :class="item" @select="emit('openSettings')">
          <Settings class="size-4" />
          {{ t('apps.settings') }}
        </DropdownMenuItem>
        <template v-if="session.can('system.manage')">
          <DropdownMenuItem :class="item" @select="confirming = 'reboot'">
            <RotateCcw class="size-4" />
            {{ t('desktop.restart') }}
          </DropdownMenuItem>
          <DropdownMenuItem :class="item" @select="confirming = 'power_off'">
            <Power class="size-4" />
            {{ t('desktop.shutDown') }}
          </DropdownMenuItem>
        </template>
        <DropdownMenuSeparator class="my-1 h-px bg-border" />
        <DropdownMenuItem :class="item" @select="emit('signOut')">
          <LogOut class="size-4" />
          {{ t('desktop.signOut') }}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenuPortal>
  </DropdownMenuRoot>

  <ConfirmDialog
    v-model:open="dialogOpen"
    :title="confirming === 'power_off' ? t('desktop.shutDownTitle') : t('desktop.restartTitle')"
    :description="
      confirming === 'power_off'
        ? t('desktop.shutDownDescription')
        : t('desktop.restartDescription')
    "
    :confirm-label="
      confirming === 'power_off' ? t('desktop.shutDownAction') : t('desktop.restartAction')
    "
    :cancel-label="t('desktop.cancel')"
    destructive
    @confirm="confirmed"
  />
</template>
