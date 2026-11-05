<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Account, AccountRole } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useAccountActions } from '../api'

/** Mirrors the server's rule; the server decides. */
const MIN_PASSWORD_CHARS = 10
const ROLES: AccountRole[] = ['user', 'readonly', 'admin']

/** Creates an account, or edits `account` if given. */
const props = defineProps<{ account?: Account; own?: boolean }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useAccountActions()
const notifications = useNotifications()

const name = ref('')
const fullName = ref('')
const role = ref<AccountRole>('user')
const enabled = ref(true)
const password = ref('')
const error = ref<string | null>(null)
const pending = ref(false)

const editing = computed(() => props.account !== undefined)

watch(open, (isOpen) => {
  if (!isOpen) return
  name.value = props.account?.name ?? ''
  fullName.value = props.account?.full_name ?? ''
  role.value = props.account?.role ?? 'user'
  enabled.value = !(props.account?.disabled ?? false)
  password.value = ''
  error.value = null
})

async function submit(): Promise<void> {
  pending.value = true
  error.value = null
  try {
    if (props.account) {
      await actions.updateAccount(props.account.id, {
        full_name: fullName.value,
        ...(props.own ? {} : { role: role.value, disabled: !enabled.value }),
      })
      notifications.notify('success', t('users.saved', { name: props.account.name }))
    } else {
      const created = await actions.createAccount({
        name: name.value.trim(),
        full_name: fullName.value || null,
        role: role.value,
        password: password.value,
      })
      notifications.notify(
        'success',
        t('users.created', { name: created.name }),
        t('users.createdHint'),
      )
    }
    open.value = false
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}
</script>

<template>
  <ModalDialog
    v-model:open="open"
    :title="editing ? t('users.editTitle', { name: account?.name }) : t('users.newTitle')"
    :description="editing ? undefined : t('users.newDescription')"
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-4" @submit.prevent="submit">
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <div v-if="!editing" class="grid gap-2">
        <Label for="account-name">{{ t('users.name') }}</Label>
        <Input
          id="account-name"
          v-model="name"
          autocomplete="off"
          autocapitalize="none"
          spellcheck="false"
          required
          maxlength="32"
        />
        <p class="text-xs text-muted-foreground">{{ t('setup.nameHint') }}</p>
      </div>
      <div class="grid gap-2">
        <Label for="account-full-name">{{ t('users.fullName') }}</Label>
        <Input id="account-full-name" v-model="fullName" autocomplete="off" maxlength="100" />
      </div>
      <div class="grid gap-2">
        <Label for="account-role">{{ t('settings.role') }}</Label>
        <select
          id="account-role"
          v-model="role"
          :disabled="own"
          class="h-9 rounded-md border bg-background px-3 text-sm shadow-xs disabled:opacity-50"
        >
          <option v-for="option in ROLES" :key="option" :value="option">
            {{ t(`settings.roles.${option}`) }}
          </option>
        </select>
        <p class="text-xs text-muted-foreground">{{ t(`users.roleHint.${role}`) }}</p>
      </div>
      <label v-if="editing" class="flex items-center gap-2 text-sm">
        <input v-model="enabled" type="checkbox" class="size-4 accent-primary" :disabled="own" />
        {{ t('users.enabled') }}
      </label>
      <p v-if="own" class="text-xs text-muted-foreground">{{ t('users.ownHint') }}</p>
      <div v-if="!editing" class="grid gap-2">
        <Label for="account-password">{{ t('users.initialPassword') }}</Label>
        <Input
          id="account-password"
          v-model="password"
          type="password"
          autocomplete="new-password"
          :minlength="MIN_PASSWORD_CHARS"
          required
        />
        <p class="text-xs text-muted-foreground">
          {{ t('users.initialPasswordHint', { min: MIN_PASSWORD_CHARS }) }}
        </p>
      </div>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button type="submit" :disabled="pending">
          {{ editing ? t('users.save') : t('users.create') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
