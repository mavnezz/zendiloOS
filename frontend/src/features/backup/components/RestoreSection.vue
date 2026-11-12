<script setup lang="ts">
import { ChevronRight, CornerLeftUp, File, Folder, FolderOpen, LoaderCircle } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BackupEntry, BackupTask, BackupVersions } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { FolderPicker } from '@/shared/ui/folder-picker'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'

import { browse, useBackupActions, versions } from '../api'
import { joinPath, parentPath, restorePaths, splitPlace } from '../lib/backup'
import { placeText } from '../lib/describe'
import { useNames } from '../names'

/** Restores from a task's backup: `task` the one chosen elsewhere. */
const props = defineProps<{ tasks: BackupTask[]; task: number | null }>()

const { t } = useI18n()
const actions = useBackupActions()
const jobs = useJobsStore()
const { shareNames, shareIds, driveLabels } = useNames()

const chosenTask = ref<number | null>(props.task ?? props.tasks[0]?.id ?? null)
const held = ref<BackupVersions | null>(null)
/** A version by its time; `current` for a mirror's copy. */
const version = ref<string>('current')
const loading = ref(false)
const failure = ref<string | null>(null)
const notice = ref<string | null>(null)
const everything = ref(true)
const folder = ref('')
const entries = ref<BackupEntry[]>([])
const chosen = ref<string[]>([])
const elsewhere = ref(false)
const destination = ref('')
const picking = ref(false)
const existing = ref<'skip' | 'replace'>('skip')
const pending = ref(false)

const task = computed(
  () => props.tasks.find((candidate) => candidate.id === chosenTask.value) ?? null,
)
const fromServer = computed(() => task.value?.source.kind === 'server')
const versionTime = computed(() => (version.value === 'current' ? null : Number(version.value)))
const original = computed(() =>
  task.value
    ? placeText(t, task.value.source, task.value.server, shareNames.value, driveLabels.value)
    : '',
)

async function look(): Promise<void> {
  held.value = null
  entries.value = []
  chosen.value = []
  folder.value = ''
  failure.value = null
  if (chosenTask.value == null) return
  loading.value = true
  try {
    held.value = await versions(chosenTask.value)
    version.value = held.value.current ? 'current' : String(held.value.versions[0] ?? 'current')
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    loading.value = false
  }
}

async function open(path: string): Promise<void> {
  if (chosenTask.value == null) return
  failure.value = null
  try {
    entries.value = await browse(chosenTask.value, versionTime.value, path)
    folder.value = path
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}

watch(
  () => props.task,
  (id) => id != null && (chosenTask.value = id),
)
// The tasks come after the page: the first is chosen unless another was.
watch(
  () => props.tasks,
  (tasks) => {
    if (chosenTask.value == null && tasks.length) chosenTask.value = tasks[0]!.id
  },
)
watch(chosenTask, () => void look(), { immediate: true })
watch([everything, version], () => {
  chosen.value = []
  if (!everything.value && held.value) void open('')
})
watch(fromServer, (server) => server && (elsewhere.value = true), { immediate: true })

function toggle(path: string): void {
  chosen.value = chosen.value.includes(path)
    ? chosen.value.filter((other) => other !== path)
    : [...chosen.value, path]
}

const hasVersion = computed(() =>
  Boolean(held.value && (held.value.current || held.value.versions.length)),
)
const ready = computed(
  () =>
    hasVersion.value &&
    (everything.value || chosen.value.length > 0) &&
    (!elsewhere.value || Boolean(destination.value)),
)

async function start(): Promise<void> {
  if (!task.value || !ready.value) return
  pending.value = true
  failure.value = null
  notice.value = null
  try {
    const target = splitPlace(destination.value)
    const job = await actions.restore(task.value.id, {
      version: versionTime.value,
      paths: everything.value ? [] : restorePaths(chosen.value),
      to: elsewhere.value
        ? { kind: 'share', share_id: shareIds.value.get(target.share) ?? 0, path: target.path }
        : null,
      existing: existing.value,
    })
    jobs.update(job)
    notice.value = t('backup.restore.started')
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    pending.value = false
  }
}
</script>

<!-- A task, a version, all or some of it, where to, and what to do with what is there. -->
<template>
  <div class="grid max-w-3xl gap-5">
    <h2 class="text-xl font-semibold">{{ t('backup.restore.title') }}</h2>
    <p v-if="!tasks.length" class="text-sm text-muted-foreground">
      {{ t('backup.restore.noTasks') }}
    </p>
    <template v-else>
      <div class="grid max-w-md gap-1.5">
        <Label for="restore-task">{{ t('backup.restore.task') }}</Label>
        <NativeSelect
          id="restore-task"
          :model-value="chosenTask ?? ''"
          @update:model-value="(id) => (chosenTask = Number(id))"
        >
          <option v-for="candidate in tasks" :key="candidate.id" :value="candidate.id">
            {{ candidate.name }}
          </option>
        </NativeSelect>
      </div>
      <p v-if="loading" class="flex items-center gap-2 text-sm text-muted-foreground">
        <LoaderCircle class="size-4 animate-spin" /> {{ t('backup.restore.loading') }}
      </p>
      <p v-else-if="held && !hasVersion" class="text-sm text-muted-foreground">
        {{ t('backup.restore.noVersions') }}
      </p>
      <template v-if="hasVersion && held">
        <div class="grid max-w-md gap-1.5">
          <Label for="restore-version">{{ t('backup.restore.version') }}</Label>
          <NativeSelect id="restore-version" v-model="version">
            <option v-if="held.current" value="current">{{ t('backup.restore.current') }}</option>
            <option v-for="at in held.versions" :key="at" :value="String(at)">
              {{ formatDateTime(at * 1000) }}
            </option>
          </NativeSelect>
        </div>

        <fieldset class="grid gap-2">
          <legend class="mb-1 text-sm font-medium">{{ t('backup.restore.contents') }}</legend>
          <label class="flex items-center gap-2 text-sm"
            ><input v-model="everything" type="radio" :value="true" />
            {{ t('backup.restore.all') }}</label
          >
          <label class="flex items-center gap-2 text-sm"
            ><input v-model="everything" type="radio" :value="false" />
            {{ t('backup.restore.some') }}</label
          >
        </fieldset>
        <div
          v-if="!everything"
          class="grid gap-2 rounded-xl border p-3"
          data-testid="restore-browser"
        >
          <div class="flex items-center gap-2 text-sm">
            <Button
              type="button"
              size="sm"
              variant="ghost"
              :disabled="!folder"
              @click="open(parentPath(folder))"
            >
              <CornerLeftUp /> {{ t('backup.restore.up') }}
            </Button>
            <span class="truncate text-muted-foreground">/{{ folder || '' }}</span>
            <span class="ml-auto text-xs text-muted-foreground">{{
              t('backup.restore.chosen', chosen.length)
            }}</span>
          </div>
          <p v-if="!entries.length" class="p-2 text-sm text-muted-foreground">
            {{ t('backup.restore.emptyFolder') }}
          </p>
          <ul class="grid max-h-72 overflow-y-auto text-sm">
            <li
              v-for="entry in entries"
              :key="entry.name"
              class="flex items-center gap-2 rounded px-1 py-1 hover:bg-accent/50"
            >
              <input
                type="checkbox"
                :aria-label="entry.name"
                :checked="chosen.includes(joinPath(folder, entry.name))"
                @change="toggle(joinPath(folder, entry.name))"
              />
              <Folder v-if="entry.kind === 'folder'" class="size-4 text-muted-foreground" />
              <File v-else class="size-4 text-muted-foreground" />
              <button
                v-if="entry.kind === 'folder'"
                type="button"
                class="flex min-w-0 flex-1 items-center gap-1 text-left"
                @click="open(joinPath(folder, entry.name))"
              >
                <span class="truncate">{{ entry.name }}</span>
                <ChevronRight class="size-3.5 text-muted-foreground" />
              </button>
              <span v-else class="min-w-0 flex-1 truncate">{{ entry.name }}</span>
              <span v-if="entry.kind === 'file'" class="text-xs text-muted-foreground">{{
                formatBytes(entry.bytes)
              }}</span>
            </li>
          </ul>
        </div>

        <fieldset class="grid gap-2">
          <legend class="mb-1 text-sm font-medium">{{ t('backup.restore.to') }}</legend>
          <label v-if="!fromServer" class="flex items-center gap-2 text-sm"
            ><input v-model="elsewhere" type="radio" :value="false" />
            {{ t('backup.restore.original', { place: original }) }}</label
          >
          <p v-else class="text-xs text-muted-foreground">
            {{ t('backup.restore.serverOriginal') }}
          </p>
          <label class="flex items-center gap-2 text-sm"
            ><input v-model="elsewhere" type="radio" :value="true" />
            {{ t('backup.restore.elsewhere') }}</label
          >
          <div v-if="elsewhere" class="flex items-center gap-2 pl-6">
            <Input
              :model-value="destination"
              readonly
              class="max-w-md"
              :aria-label="t('backup.restore.elsewhere')"
            />
            <Button type="button" variant="outline" @click="picking = true"
              ><FolderOpen /> {{ t('backup.wizard.choose') }}</Button
            >
          </div>
          <p class="text-xs text-muted-foreground">{{ t('backup.restore.intoShareHint') }}</p>
        </fieldset>

        <fieldset class="grid gap-2">
          <legend class="mb-1 text-sm font-medium">{{ t('backup.restore.existing') }}</legend>
          <label class="flex items-center gap-2 text-sm"
            ><input v-model="existing" type="radio" value="skip" />
            {{ t('backup.restore.skip') }}</label
          >
          <label class="flex items-center gap-2 text-sm"
            ><input v-model="existing" type="radio" value="replace" />
            {{ t('backup.restore.replace') }}</label
          >
        </fieldset>

        <Button class="w-fit" :disabled="!ready || pending" @click="start">{{
          t('backup.restore.start')
        }}</Button>
      </template>
      <Alert v-if="notice">{{ notice }}</Alert>
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    </template>
    <FolderPicker
      v-model:open="picking"
      :value="destination"
      writable
      @pick="(path) => ((destination = path), (picking = false))"
    />
  </div>
</template>
