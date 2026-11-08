<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerVolume, VolumeBackup } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatDateTime } from '@/shared/lib/format'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { ModalDialog } from '@/shared/ui/dialog'

import { restoreBackup, useRefreshDocker } from '../api'

const props = defineProps<{ backup: VolumeBackup | null; volumes: DockerVolume[] }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ started: [] }>()

const { t } = useI18n()
const jobs = useJobsStore()
const notifications = useNotifications()
const refresh = useRefreshDocker()

const into = ref<'own_name' | 'existing'>('own_name')
const other = ref('')
const stop = ref(true)
const confirming = ref(false)
const busy = ref(false)
const failure = ref<string | null>(null)

watch(open, (isOpen) => {
  if (!isOpen) return
  into.value = 'own_name'
  other.value = ''
  stop.value = true
  failure.value = null
})

const at = computed(() => (props.backup ? formatDateTime(props.backup.started_at * 1000) : ''))
/** Other volumes it can go into: local ones whose data zendiloOS reaches. */
const others = computed(() =>
  props.volumes.filter((volume) => volume.backed_up_here && volume.name !== props.backup?.volume),
)
const target = computed(() =>
  into.value === 'own_name' ? (props.backup?.volume ?? '') : other.value,
)
const running = computed(
  () =>
    props.volumes
      .find((volume) => volume.name === target.value)
      ?.used_by.filter((user) => user.running)
      .map((user) => user.name) ?? [],
)
const ready = computed(() => !!target.value && (running.value.length === 0 || stop.value))

async function start(): Promise<void> {
  const backup = props.backup
  if (!backup || !ready.value) return
  busy.value = true
  failure.value = null
  try {
    const job = await restoreBackup(backup.id, {
      target: into.value,
      volume: into.value === 'existing' ? other.value : null,
      stop_containers: running.value.length > 0 && stop.value,
    })
    jobs.update(job)
    notifications.notify(
      'info',
      t('docker.volumes.restore.title', { name: target.value }),
      t('docker.volumes.started'),
    )
    await refresh()
    open.value = false
    emit('started')
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}
</script>

<!-- Where a backup is restored to, and what has to stop for it. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('docker.volumes.restore.title', { name: backup?.volume ?? '' })"
    :description="t('docker.volumes.restore.from', { at })"
    :close-label="t('desktop.close')"
  >
    <form id="restore-backup" class="grid gap-3" @submit.prevent="confirming = true">
      <fieldset class="grid gap-2">
        <legend class="mb-1 text-sm font-medium">{{ t('docker.volumes.restore.target') }}</legend>
        <label class="flex items-start gap-2 text-sm">
          <input
            v-model="into"
            type="radio"
            name="restore-into"
            value="own_name"
            class="mt-0.5 size-4 accent-primary"
          />
          <span class="grid gap-0.5">
            <span>{{ t('docker.volumes.restore.own', { name: backup?.volume ?? '' }) }}</span>
            <span class="text-xs text-muted-foreground">{{
              t('docker.volumes.restore.ownHint')
            }}</span>
          </span>
        </label>
        <label class="flex items-center gap-2 text-sm">
          <input
            v-model="into"
            type="radio"
            name="restore-into"
            value="existing"
            class="size-4 accent-primary"
            :disabled="!others.length"
          />
          {{ t('docker.volumes.restore.existing') }}
        </label>
        <select
          v-if="into === 'existing'"
          v-model="other"
          class="ml-6 h-9 rounded-md border bg-background px-2 text-sm"
          :aria-label="t('docker.volumes.restore.choose')"
        >
          <option value="" disabled>{{ t('docker.volumes.restore.choose') }}</option>
          <option v-for="volume in others" :key="volume.name" :value="volume.name">
            {{ volume.name }}
          </option>
        </select>
      </fieldset>

      <template v-if="running.length">
        <Alert>{{
          t('docker.volumes.restore.inUse', { name: target, names: running.join(', ') })
        }}</Alert>
        <label class="flex items-center gap-2 text-sm">
          <input v-model="stop" type="checkbox" class="size-4 accent-primary" />
          {{ t('docker.volumes.restore.stop') }}
        </label>
      </template>
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    </form>

    <template #footer>
      <Button type="button" variant="secondary" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button type="submit" form="restore-backup" :disabled="busy || !ready">{{
        t('docker.volumes.restore.start')
      }}</Button>
    </template>
    <ConfirmDialog
      v-model:open="confirming"
      :title="t('docker.volumes.restore.confirmTitle', { name: target })"
      :description="t('docker.volumes.restore.confirmText', { name: target, at })"
      :confirm-label="t('docker.volumes.restore.start')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @confirm="start"
    />
  </ModalDialog>
</template>
