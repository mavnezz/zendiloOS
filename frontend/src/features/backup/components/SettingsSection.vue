<script setup lang="ts">
import { Cloud, Plus, Trash2 } from 'lucide-vue-next'
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BackupCloudAccount } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useBackupAccounts, useBackupActions, useBackupSettings } from '../api'
import SignInDialog from './SignInDialog.vue'

const { t } = useI18n()
const { data: settings, error } = useBackupSettings()
const { data: accounts } = useBackupAccounts()
const actions = useBackupActions()
const signingIn = ref(false)
const removing = ref<BackupCloudAccount | null>(null)
const accountFailure = ref<string | null>(null)
const removeOpen = ref(false)

function askRemove(account: BackupCloudAccount): void {
  removing.value = account
  removeOpen.value = true
}

async function remove(): Promise<void> {
  const account = removing.value
  if (!account) return
  accountFailure.value = null
  try {
    await actions.removeAccount(account.id)
  } catch (problem) {
    accountFailure.value = problemMessage(problem)
  }
}
const logDays = ref(90)
const bandwidth = ref(0)
const pending = ref(false)
const failure = ref<string | null>(null)
const saved = ref(false)

watch(
  settings,
  (current) => {
    if (!current) return
    logDays.value = current.log_days
    bandwidth.value = current.bwlimit_kib
  },
  { immediate: true },
)

async function save(): Promise<void> {
  pending.value = true
  failure.value = null
  saved.value = false
  try {
    await actions.settings({
      log_days: Number(logDays.value),
      bwlimit_kib: Number(bandwidth.value),
    })
    saved.value = true
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    pending.value = false
  }
}
</script>

<template>
  <div class="grid max-w-xl gap-10">
    <form class="grid gap-5" @submit.prevent="save">
      <h2 class="display text-xl">{{ t('backup.settings.title') }}</h2>
      <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
      <div class="grid gap-1.5">
        <Label for="backup-log-days">{{ t('backup.settings.logDays') }}</Label>
        <Input id="backup-log-days" v-model="logDays" type="number" class="w-32" />
        <p class="text-xs text-muted-foreground">{{ t('backup.settings.logDaysHint') }}</p>
      </div>
      <div class="grid gap-1.5">
        <Label for="backup-bandwidth">{{ t('backup.settings.bandwidth') }}</Label>
        <Input id="backup-bandwidth" v-model="bandwidth" type="number" class="w-32" />
        <p class="text-xs text-muted-foreground">{{ t('backup.settings.bandwidthHint') }}</p>
      </div>
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
      <p v-if="saved" class="text-sm text-emerald-600 dark:text-emerald-400">
        {{ t('backup.settings.saved') }}
      </p>
      <Button type="submit" class="w-fit" :disabled="pending || !settings">{{
        t('backup.settings.save')
      }}</Button>
    </form>

    <section class="grid gap-4" data-testid="backup-accounts">
      <div class="flex items-center gap-3">
        <h3 class="flex-1 text-lg font-semibold">{{ t('backup.accounts.title') }}</h3>
        <Button type="button" variant="outline" @click="signingIn = true"
          ><Plus /> {{ t('backup.accounts.add') }}</Button
        >
      </div>
      <p class="text-sm text-muted-foreground">{{ t('backup.accounts.hint') }}</p>
      <p v-if="accounts && !accounts.length" class="text-sm text-muted-foreground">
        {{ t('backup.accounts.empty') }}
      </p>
      <ul v-else class="grid gap-2">
        <li
          v-for="account in accounts"
          :key="account.id"
          class="flex items-center gap-3 rounded-lg border p-3 text-sm"
        >
          <Cloud class="size-5 text-muted-foreground" />
          <span class="grid min-w-0 flex-1">
            <span class="font-medium">{{ account.name }}</span>
            <span class="text-xs text-muted-foreground"
              >{{ t(`backup.accounts.providers.${account.provider}`)
              }}{{ account.client_id ? ` · ${t('backup.accounts.ownClient')}` : '' }}</span
            >
          </span>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            :aria-label="t('backup.accounts.remove', { name: account.name })"
            @click="askRemove(account)"
            ><Trash2
          /></Button>
        </li>
      </ul>
      <Alert v-if="accountFailure" variant="destructive">{{ accountFailure }}</Alert>
    </section>
    <SignInDialog v-model:open="signingIn" />
    <ConfirmDialog
      v-model:open="removeOpen"
      :title="t('backup.accounts.removeTitle', { name: removing?.name ?? '' })"
      :description="t('backup.accounts.removeText')"
      :confirm-label="t('backup.accounts.removeConfirm')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @confirm="remove"
    />
  </div>
</template>
