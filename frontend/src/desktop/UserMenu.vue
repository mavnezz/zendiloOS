<script setup lang="ts">
import { LogOut, Power, RotateCcw, Settings } from 'lucide-vue-next'
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
      class="flex size-11 items-center justify-center rounded-tile outline-none transition hover:bg-foreground/10 focus-visible:ring-2 focus-visible:ring-ring data-[state=open]:bg-foreground/10"
      :aria-label="t('desktop.account')"
    >
      <span
        class="flex size-8 items-center justify-center rounded-full bg-primary/15 text-xs font-semibold text-primary uppercase ring-1 ring-primary/30"
      >
        {{ account?.name.slice(0, 2) ?? '?' }}
      </span>
    </DropdownMenuTrigger>
    <DropdownMenuPortal>
      <DropdownMenuContent
        side="right"
        align="end"
        :side-offset="12"
        class="glass glass-strong z-[900] min-w-52 rounded-xl p-1"
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
