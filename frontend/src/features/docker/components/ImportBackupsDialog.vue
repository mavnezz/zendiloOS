<script setup lang="ts">
import { CircleCheck, CircleX, FolderOpen, LoaderCircle } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { FolderPicker, useShareList } from '@/shared/ui/folder-picker'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { importBackups, useRefreshDocker } from '../api'
import { planTarget } from '../lib/volumes'

const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const jobs = useJobsStore()
const refresh = useRefreshDocker()
const { data: shares } = useShareList()

const folder = ref('')
const browsing = ref(false)
const starting = ref(false)
const failure = ref<string | null>(null)
/** The import started from here, followed over the live socket. */
const jobId = ref<number | null>(null)

const writable = computed(() => (shares.value ?? []).filter((share) => share.writable))
const target = computed(() => planTarget(folder.value, writable.value))
const job = computed(() => (jobId.value === null ? undefined : jobs.jobs.get(jobId.value)))
const running = computed(() => job.value?.state === 'running')
const lines = computed(() => job.value?.output ?? [])

// Each opening starts afresh, unless an import started here still runs.
watch(open, (isOpen) => {
  if (!isOpen || running.value) return
  folder.value = ''
  failure.value = null
  jobId.value = null
})

// What it recorded shows in the list once it ended.
watch(
  () => job.value?.state,
  (state, before) => {
    if (before === 'running' && state !== 'running') void refresh()
  },
)

async function start(): Promise<void> {
  if (!target.value) return
  starting.value = true
  failure.value = null
  try {
    const started = await importBackups(target.value)
    jobs.update(started)
    jobId.value = started.id
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    starting.value = false
  }
}

function again(): void {
  jobId.value = null
  folder.value = ''
}

const hint = 'text-xs text-muted-foreground'
</script>

<!-- Backups found in a folder recorded, for when zendiloOS lost them: which were, and which were refused and why. -->
<template>
  <ModalDialog
    v-model:open="open"
    wide
    :title="t('docker.backups.importDialog.title')"
    :description="t('docker.backups.importDialog.hint')"
    :close-label="t('desktop.close')"
  >
    <form v-if="!job" id="import-backups" class="grid gap-3" @submit.prevent="start">
      <div class="grid gap-2">
        <Label for="import-folder" class="text-sm font-medium">{{
          t('docker.backups.importDialog.folder')
        }}</Label>
        <div class="flex min-w-0 gap-2">
          <Input
            id="import-folder"
            v-model="folder"
            class="min-w-0 flex-1 font-mono"
            autocomplete="off"
            spellcheck="false"
            :aria-invalid="folder.trim() !== '' && !target"
          />
          <Button type="button" variant="outline" class="shrink-0" @click="browsing = true">
            <FolderOpen />
            {{ t('docker.volumes.plan.browse') }}
          </Button>
        </div>
        <p :class="folder.trim() !== '' && !target ? 'text-xs text-destructive' : hint">
          {{ t('docker.volumes.plan.noFolder') }}
        </p>
      </div>
      <p :class="hint">{{ t('docker.backups.importDialog.rules') }}</p>
      <p :class="hint">{{ t('docker.backups.importDialog.noPlan') }}</p>
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    </form>
    <template v-else>
      <p class="text-sm font-medium">{{ t('docker.backups.importDialog.output') }}</p>
      <pre
        data-testid="import-output"
        class="max-h-72 overflow-auto rounded-lg bg-zinc-900 p-3 font-mono text-[12px] leading-5 whitespace-pre-wrap text-zinc-100"
        >{{ lines.length ? lines.join('\n') : t('docker.backups.importDialog.waiting') }}</pre>
      <p class="flex items-center gap-2 text-sm" role="status">
        <template v-if="running">
          <LoaderCircle class="size-4 animate-spin text-primary" />
          {{ t('docker.backups.importDialog.running') }}
        </template>
        <template v-else-if="job.state === 'succeeded'">
          <CircleCheck class="size-4 text-success" />
          {{ t('docker.backups.importDialog.succeeded') }}
        </template>
        <template v-else>
          <CircleX class="size-4 text-destructive" />
          <span>
            {{ t('docker.backups.importDialog.failed') }}
            <span v-if="job.error_detail" class="block font-mono text-xs text-muted-foreground">{{
              job.error_detail
            }}</span>
          </span>
        </template>
      </p>
    </template>

    <template #footer>
      <template v-if="!job">
        <Button type="button" variant="secondary" @click="open = false">{{
          t('desktop.cancel')
        }}</Button>
        <Button type="submit" form="import-backups" :disabled="starting || !target">{{
          t('docker.backups.importDialog.start')
        }}</Button>
      </template>
      <template v-else>
        <Button v-if="!running" type="button" variant="secondary" @click="again">{{
          t('docker.backups.importDialog.again')
        }}</Button>
        <Button type="button" @click="open = false">{{ t('desktop.close') }}</Button>
      </template>
    </template>
    <FolderPicker v-model:open="browsing" :value="folder" writable @pick="folder = $event" />
  </ModalDialog>
</template>
