<script setup lang="ts">
import {
  KeyRound,
  ListFilter,
  Pencil,
  Power,
  ShieldCheck,
  ShieldOff,
  Trash2,
  UserPlus,
} from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Account } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { type Action, ActionMenu } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { Avatar } from '@/shared/ui/avatar'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { Input } from '@/shared/ui/input'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useAccountActions, useAccounts } from '../api'
import AccountWizard from './AccountWizard.vue'
import ResetPasswordDialog from './ResetPasswordDialog.vue'

const props = defineProps<{ ownAccountId: number | undefined }>()

const { t } = useI18n()
const accountActions = useAccountActions()
const notifications = useNotifications()
const { data: accounts, error, isPending } = useAccounts()

const filter = ref('')
const creating = ref(false)
const editing = ref<Account | null>(null)
const resetting = ref<Account | null>(null)
const deleting = ref<Account | null>(null)
const resettingTwoFactor = ref<Account | null>(null)

const isOwn = (account: Account) => account.id === props.ownAccountId
const shown = computed(() => {
  const needle = filter.value.trim().toLowerCase()
  return (accounts.value ?? []).filter(
    (account) =>
      !needle ||
      account.name.includes(needle) ||
      [account.full_name, account.description, account.email].some((text) =>
        (text ?? '').toLowerCase().includes(needle),
      ),
  )
})

function status(account: Account): {
  label: string
  variant: 'success' | 'destructive' | 'warning'
} {
  if (account.disabled) return { label: t('users.disabled'), variant: 'destructive' }
  if (account.password_change_required)
    return { label: t('users.mustChangePassword'), variant: 'warning' }
  return { label: t('people.normal'), variant: 'success' }
}

function actionsFor(account: Account): Action[] {
  const own = isOwn(account)
  return [
    { id: 'edit', label: t('users.edit'), icon: Pencil },
    { id: 'password', label: t('users.resetPassword'), icon: KeyRound, disabled: own },
    {
      id: 'two-factor',
      label: t('users.resetTwoFactor'),
      icon: ShieldOff,
      disabled: own || !account.two_factor,
    },
    {
      id: 'toggle',
      label: account.disabled ? t('people.enable') : t('people.disable'),
      icon: Power,
      disabled: own,
    },
    { id: 'delete', label: t('users.delete'), icon: Trash2, destructive: true, disabled: own },
  ]
}

async function run(account: Account, action: string): Promise<void> {
  if (action === 'edit') editing.value = account
  else if (action === 'password') resetting.value = account
  else if (action === 'two-factor') resettingTwoFactor.value = account
  else if (action === 'delete') deleting.value = account
  else if (action === 'toggle') {
    try {
      const updated = await accountActions.updateAccount(account.id, {
        disabled: !account.disabled,
      })
      // Disabled, it is kept out of the shares' files too (NFS): they
      // follow in the background.
      if (updated.pending_shares.length) {
        notifications.notify(
          'success',
          t('users.saved', { name: account.name }),
          t('people.sharesFollow', { shares: updated.pending_shares.join(', ') }),
        )
      }
    } catch (failure) {
      notifications.notify('error', t('people.toggleFailed'), problemMessage(failure))
    }
  }
}

async function resetTwoFactor(): Promise<void> {
  const account = resettingTwoFactor.value
  if (!account) return
  try {
    await accountActions.resetTwoFactor(account.id)
    notifications.notify('success', t('users.twoFactorReset', { name: account.name }))
  } catch (failure) {
    notifications.notify('error', t('users.resetTwoFactorFailed'), problemMessage(failure))
  }
}

async function remove(): Promise<void> {
  const account = deleting.value
  if (!account) return
  try {
    await accountActions.deleteAccount(account.id)
    notifications.notify('success', t('users.deleted', { name: account.name }))
  } catch (failure) {
    notifications.notify('error', t('users.deleteFailed'), problemMessage(failure))
  }
}
</script>

<!-- Every account: who it is, what it may do, whether it can sign in. -->
<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <Button @click="creating = true">
        <UserPlus />
        {{ t('people.add') }}
      </Button>
      <div class="relative ml-auto w-full max-w-60">
        <ListFilter
          class="absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="filter"
          class="pl-8"
          :placeholder="t('people.filter')"
          :aria-label="t('people.filter')"
        />
      </div>
    </div>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <div class="overflow-hidden rounded-xl border bg-card">
      <Table>
        <TableHeader class="bg-muted/50">
          <TableRow class="hover:bg-transparent">
            <TableHead>{{ t('users.name') }}</TableHead>
            <TableHead class="hidden @2xl:table-cell">{{ t('people.description') }}</TableHead>
            <TableHead>{{ t('settings.role') }}</TableHead>
            <TableHead>{{ t('people.status') }}</TableHead>
            <TableHead class="w-12"
              ><span class="sr-only">{{ t('people.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="account in shown" :key="account.id">
            <TableCell>
              <span class="flex min-w-0 items-center gap-2.5">
                <Avatar :name="account.name" />
                <span class="min-w-0">
                  <span class="flex items-center gap-1.5">
                    <span class="truncate font-medium">{{ account.name }}</span>
                    <Badge v-if="isOwn(account)">{{ t('people.me') }}</Badge>
                    <Badge
                      v-if="account.two_factor"
                      variant="success"
                      :title="t('users.twoFactorHint')"
                      data-testid="two-factor"
                    >
                      <ShieldCheck />
                      {{ t('users.twoFactor') }}
                    </Badge>
                  </span>
                  <span
                    v-if="account.full_name"
                    class="block truncate text-xs text-muted-foreground"
                  >
                    {{ account.full_name }}
                  </span>
                </span>
              </span>
            </TableCell>
            <TableCell class="hidden max-w-64 truncate text-muted-foreground @2xl:table-cell">
              {{ account.description ?? '–' }}
            </TableCell>
            <TableCell>{{ t(`settings.roles.${account.role}`) }}</TableCell>
            <TableCell>
              <Badge :variant="status(account).variant">{{ status(account).label }}</Badge>
            </TableCell>
            <TableCell class="text-right">
              <ActionMenu
                :actions="actionsFor(account)"
                :label="t('people.actionsFor', { name: account.name })"
                @select="(action) => run(account, action)"
              />
            </TableCell>
          </TableRow>
          <TableRow v-if="!isPending && !shown.length" class="hover:bg-transparent">
            <TableCell colspan="5" class="py-8 text-center text-muted-foreground">
              {{ filter ? t('people.nothingFound') : t('users.none') }}
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <AccountWizard v-model:open="creating" />
    <AccountWizard
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
      :open="resettingTwoFactor !== null"
      :title="t('users.resetTwoFactorTitle', { name: resettingTwoFactor?.name })"
      :description="t('users.resetTwoFactorDescription', { name: resettingTwoFactor?.name })"
      :confirm-label="t('users.resetTwoFactor')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open) => !open && (resettingTwoFactor = null)"
      @confirm="resetTwoFactor"
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
