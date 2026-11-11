<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useBackupActions, useBackupSettings } from '../api'

const { t } = useI18n()
const { data: settings, error } = useBackupSettings()
const actions = useBackupActions()
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
  <form class="grid max-w-xl gap-5" @submit.prevent="save">
    <h2 class="text-xl font-semibold">{{ t('backup.settings.title') }}</h2>
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
</template>
