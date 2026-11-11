<script setup lang="ts">
import { FolderOpen, ShieldAlert } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { FolderPicker } from '@/shared/ui/folder-picker'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'

import { useShareReaders } from '../api'
import { type ServerForm, type TaskForm, splitPlace } from '../lib/backup'
import { useNames } from '../names'
import ServerFields from './ServerFields.vue'

/** The name, the source and the target of a task, and its server if any. */
const props = defineProps<{ folder: string; task?: number; passwordSet?: boolean }>()
const form = defineModel<TaskForm>({ required: true })

const { t } = useI18n()
const { shareIds, drives } = useNames()
const picking = ref<'source' | 'target' | null>(null)
const pickerOpen = computed({
  get: () => picking.value !== null,
  set: (open) => {
    if (!open) picking.value = null
  },
})

const set = <K extends keyof TaskForm>(name: K, value: TaskForm[K]) =>
  (form.value = { ...form.value, [name]: value })
const fromNas = computed(() => form.value.kind !== 'pull')
const intoNas = computed(() => form.value.kind !== 'push')
const mounted = computed(() => (drives.value ?? []).filter((drive) => drive.state === 'mounted'))
const drive = computed(() => mounted.value.find((candidate) => candidate.id === form.value.drive))
const targetShare = computed(() =>
  intoNas.value && form.value.targetKind === 'share' && form.value.target
    ? (shareIds.value.get(splitPlace(form.value.target).share) ?? null)
    : null,
)
const { data: readers } = useShareReaders(() => targetShare.value)
const who = computed(() => {
  const all = [
    ...(readers.value?.accounts ?? []),
    ...(readers.value?.groups ?? []).map((group) => `@${group}`),
    ...(readers.value?.nfs ?? []),
  ]
  return all.join(', ')
})

function pick(path: string): void {
  if (picking.value === 'source') set('source', path)
  else if (picking.value === 'target') set('target', path)
  picking.value = null
}
</script>

<template>
  <div class="grid gap-5">
    <div class="grid max-w-md gap-1.5">
      <Label for="backup-name">{{ t('backup.wizard.name') }}</Label>
      <Input
        id="backup-name"
        :model-value="form.name"
        :placeholder="t('backup.wizard.namePlaceholder')"
        maxlength="64"
        @update:model-value="(value) => set('name', String(value))"
      />
    </div>

    <div v-if="fromNas" class="grid gap-1.5">
      <Label>{{ t('backup.wizard.source') }}</Label>
      <div class="flex items-center gap-2">
        <Input
          :model-value="form.source"
          readonly
          class="flex-1"
          :aria-label="t('backup.wizard.source')"
        />
        <Button type="button" variant="outline" @click="picking = 'source'"
          ><FolderOpen /> {{ t('backup.wizard.choose') }}</Button
        >
      </div>
      <p class="text-xs text-muted-foreground">{{ t('backup.wizard.sourceHint') }}</p>
    </div>

    <ServerFields
      v-if="form.kind !== 'local'"
      :model-value="form.server"
      :path="form.serverPath"
      :task="task"
      :password-set="passwordSet"
      @update:model-value="(value: ServerForm) => set('server', value)"
    />
    <div v-if="form.kind !== 'local'" class="grid gap-1.5">
      <Label for="backup-server-path">{{ t('backup.server.folder') }}</Label>
      <Input
        id="backup-server-path"
        :model-value="form.serverPath"
        @update:model-value="(value) => set('serverPath', String(value))"
      />
      <p class="text-xs text-muted-foreground">
        {{
          form.server.transport === 'daemon'
            ? t('backup.server.folderHintDaemon')
            : t('backup.server.folderHintSsh')
        }}
      </p>
    </div>

    <div v-if="intoNas" class="grid gap-2">
      <Label>{{ t('backup.wizard.target') }}</Label>
      <div
        class="flex flex-wrap gap-4 text-sm"
        role="radiogroup"
        :aria-label="t('backup.wizard.target')"
      >
        <label
          v-for="kind in ['share', 'drive'] as const"
          :key="kind"
          class="flex items-center gap-2"
        >
          <input
            type="radio"
            name="backup-target-kind"
            :checked="form.targetKind === kind"
            @change="set('targetKind', kind)"
          />
          {{ kind === 'share' ? t('backup.wizard.targetShare') : t('backup.wizard.targetDrive') }}
        </label>
      </div>
      <div v-if="form.targetKind === 'share'" class="flex items-center gap-2">
        <Input
          :model-value="form.target"
          readonly
          class="flex-1"
          :aria-label="t('backup.wizard.targetShare')"
        />
        <Button type="button" variant="outline" @click="picking = 'target'"
          ><FolderOpen /> {{ t('backup.wizard.choose') }}</Button
        >
      </div>
      <template v-else>
        <p v-if="!mounted.length" class="text-sm text-muted-foreground">
          {{ t('backup.wizard.noDrive') }}
        </p>
        <NativeSelect
          v-else
          :model-value="form.drive"
          class="max-w-md"
          @update:model-value="(value) => set('drive', String(value))"
        >
          <option value="" disabled>{{ t('backup.wizard.choose') }}</option>
          <option v-for="choice in mounted" :key="choice.id" :value="choice.id">
            {{
              t('backup.wizard.driveFilesystem', {
                label: choice.label || choice.id,
                filesystem: choice.filesystem,
              })
            }}
          </option>
        </NativeSelect>
        <div class="grid max-w-md gap-1.5">
          <Label for="backup-drive-path">{{ t('backup.wizard.drivePath') }}</Label>
          <Input
            id="backup-drive-path"
            :model-value="form.drivePath"
            @update:model-value="(value) => set('drivePath', String(value))"
          />
        </div>
        <Alert
          v-if="drive && drive.filesystem !== 'ext4' && form.mode === 'versioned'"
          variant="destructive"
          >{{ t('backup.wizard.driveNoLinks', { filesystem: drive.filesystem }) }}</Alert
        >
      </template>
      <p class="text-xs text-muted-foreground">
        {{ t('backup.wizard.targetHint', { folder: props.folder }) }}
      </p>
      <Alert v-if="targetShare !== null && readers" data-testid="backup-readers">
        <ShieldAlert />
        {{ who ? t('backup.wizard.readers', { who }) : t('backup.wizard.readersNobody') }}
      </Alert>
    </div>

    <FolderPicker
      v-model:open="pickerOpen"
      :value="picking === 'source' ? form.source : form.target"
      :writable="picking === 'target'"
      @pick="pick"
    />
  </div>
</template>
