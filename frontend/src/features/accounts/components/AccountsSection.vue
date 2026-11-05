<script setup lang="ts">
import { KeyRound, Pencil, Trash2, UserPlus } from 'lucide-vue-next'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Account } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'

import { useAccountActions, useAccounts } from '../api'
import AccountDialog from './AccountDialog.vue'
import ResetPasswordDialog from './ResetPasswordDialog.vue'

const props = defineProps<{ ownAccountId: number | undefined }>()

const { t } = useI18n()
const actions = useAccountActions()
const notifications = useNotifications()
const { data: accounts, error, isPending } = useAccounts()

const creating = ref(false)
const editing = ref<Account | null>(null)
const resetting = ref<Account | null>(null)
const deleting = ref<Account | null>(null)

const isOwn = (account: Account) => account.id === props.ownAccountId

async function remove(): Promise<void> {
  const account = deleting.value
  if (!account) return
  try {
    await actions.deleteAccount(account.id)
    notifications.notify('success', t('users.deleted', { name: account.name }))
  } catch (failure) {
    notifications.notify('error', t('users.deleteFailed'), problemMessage(failure))
  }
}
</script>

<template>
  <div class="grid gap-3">
    <div class="flex items-center justify-between gap-2">
      <p class="text-sm text-muted-foreground">{{ t('users.intro') }}</p>
      <Button size="sm" @click="creating = true">
        <UserPlus />
        {{ t('users.new') }}
      </Button>
    </div>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <p v-else-if="!isPending && !accounts?.length" class="text-sm text-muted-foreground">
      {{ t('users.none') }}
    </p>
    <ul class="grid gap-2">
      <li
        v-for="account in accounts"
        :key="account.id"
        class="flex flex-wrap items-center gap-3 rounded-xl border bg-card p-3"
      >
        <span
          class="grid size-9 shrink-0 place-items-center rounded-full bg-primary/10 font-medium text-primary uppercase"
          aria-hidden="true"
        >
          {{ account.name.charAt(0) }}
        </span>
        <div class="min-w-0 flex-1">
          <p class="truncate font-medium">
            {{ account.full_name ?? account.name }}
            <span v-if="isOwn(account)" class="font-normal text-muted-foreground">
              ({{ t('users.you') }})
            </span>
          </p>
          <p class="truncate text-xs text-muted-foreground">{{ account.name }}</p>
        </div>
        <div class="flex flex-wrap gap-1.5">
          <Badge :variant="account.role === 'admin' ? 'default' : 'secondary'">
            {{ t(`settings.roles.${account.role}`) }}
          </Badge>
          <Badge v-if="account.disabled" variant="destructive">{{ t('users.disabled') }}</Badge>
          <Badge v-else-if="account.password_change_required" variant="warning">
            {{ t('users.mustChangePassword') }}
          </Badge>
        </div>
        <div class="flex gap-1">
          <Button
            variant="ghost"
            size="icon"
            :title="t('users.edit')"
            :aria-label="t('users.edit')"
            @click="editing = account"
          >
            <Pencil />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            :title="t('users.resetPassword')"
            :aria-label="t('users.resetPassword')"
            :disabled="isOwn(account)"
            @click="resetting = account"
          >
            <KeyRound />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            :title="t('users.delete')"
            :aria-label="t('users.delete')"
            :disabled="isOwn(account)"
            @click="deleting = account"
          >
            <Trash2 />
          </Button>
        </div>
      </li>
    </ul>

    <AccountDialog v-model:open="creating" />
    <AccountDialog
      :open="editing !== null"
      :account="editing ?? undefined"
      :own="editing !== null && isOwn(editing)"
      @update:open="(open) => !open && (editing = null)"
    />
    <ResetPasswordDialog
      v-if="resetting"
      :open="true"
      :account="resetting"
      @update:open="(open) => !open && (resetting = null)"
    />
    <ConfirmDialog
      :open="deleting !== null"
      :title="t('users.deleteTitle', { name: deleting?.name })"
      :description="t('users.deleteDescription')"
      :confirm-label="t('users.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open) => !open && (deleting = null)"
      @confirm="remove"
    />
  </div>
</template>
