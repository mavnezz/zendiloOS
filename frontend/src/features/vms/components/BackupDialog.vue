<script setup lang="ts">
import { FolderOpen, Info } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { FolderPicker, useShareList } from '@/shared/ui/folder-picker'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'

import { useBackupActions, useVmDrives } from '../api'
import { DEFAULT_FOLDER, backupTarget, folderValid } from '../lib/backups'
import { isOn } from '../lib/vms'

/** Backs one VM up into a share or onto a USB drive, as a job. */
const props = defineProps<{ vm: VirtualMachine }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const jobs = useJobsStore()
const actions = useBackupActions()
const { data: shares } = useShareList()
const { data: drives } = useVmDrives(() => open.value)

const kind = ref<'share' | 'drive'>('share')
const path = ref('')
const drive = ref('')
const driveFolder = ref(DEFAULT_FOLDER)
const browsing = ref(false)
const pending = ref(false)
const failure = ref<string | null>(null)

const writable = computed(() => (shares.value ?? []).filter((share) => share.writable))
/** Drives mounted that can be written. */
const usable = computed(() =>
  (drives.value ?? []).filter((one) => one.state === 'mounted' && !one.read_only),
)

watch(
  [open, writable],
  ([isOpen]) => {
    if (!isOpen) return
    failure.value = null
    if (!path.value && writable.value[0]) path.value = `${writable.value[0].name}/${DEFAULT_FOLDER}`
  },
  { immediate: true },
)
watch(
  usable,
  (list) => {
    if (!list.some((one) => one.id === drive.value)) drive.value = list[0]?.id ?? ''
  },
  { immediate: true },
)

const chosen = computed(() =>
  kind.value === 'share'
    ? backupTarget('share', path.value, writable.value, '')
    : backupTarget('drive', driveFolder.value, [], drive.value),
)

async function start(): Promise<void> {
  const target = chosen.value
  if (!target) return
  pending.value = true
  failure.value = null
  try {
    jobs.update(await actions.backUp(props.vm.id, target.target, target.folder))
    open.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    pending.value = false
  }
}

const hint = 'text-xs text-muted-foreground'
</script>

<!-- Where a VM's backup goes: a folder in a share or on a USB drive. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('vms.backup.title', { name: vm.name })"
    :description="t('vms.backup.description')"
    :close-label="t('desktop.close')"
  >
    <form id="vm-backup" class="grid gap-4" @submit.prevent="start">
      <fieldset class="grid gap-2">
        <legend class="mb-1 text-sm font-medium">{{ t('vms.backup.to') }}</legend>
        <div role="radiogroup" class="flex gap-4 text-sm">
          <label class="flex items-center gap-2">
            <input
              v-model="kind"
              type="radio"
              name="vm-backup-to"
              value="share"
              class="accent-primary"
            />
            {{ t('vms.backup.share') }}
          </label>
          <label class="flex items-center gap-2">
            <input
              v-model="kind"
              type="radio"
              name="vm-backup-to"
              value="drive"
              class="accent-primary"
            />
            {{ t('vms.backup.drive') }}
          </label>
        </div>
      </fieldset>

      <div v-if="kind === 'share'" class="grid gap-2">
        <Label for="vm-backup-folder">{{ t('vms.backup.folder') }}</Label>
        <div class="flex min-w-0 gap-2">
          <Input
            id="vm-backup-folder"
            v-model="path"
            class="min-w-0 flex-1 font-mono"
            autocomplete="off"
            spellcheck="false"
            :aria-invalid="path.trim() !== '' && !chosen"
          />
          <Button type="button" variant="outline" class="shrink-0" @click="browsing = true">
            <FolderOpen />
            {{ t('vms.backup.browse') }}
          </Button>
        </div>
        <p :class="path.trim() !== '' && !chosen ? 'text-xs text-destructive' : hint">
          {{ t('vms.backup.noFolder') }}
        </p>
      </div>
      <template v-else>
        <p
          v-if="!usable.length"
          class="text-sm text-muted-foreground"
          data-testid="vm-backup-no-drives"
        >
          {{ t('vms.backup.noDrives') }}
        </p>
        <template v-else>
          <div class="grid gap-2">
            <Label for="vm-backup-drive">{{ t('vms.backup.driveChoice') }}</Label>
            <NativeSelect id="vm-backup-drive" v-model="drive">
              <option v-for="one in usable" :key="one.id" :value="one.id">
                {{ one.label ? `${one.label} (${one.id})` : one.id }}
              </option>
            </NativeSelect>
          </div>
          <div class="grid gap-2">
            <Label for="vm-backup-drive-folder">{{ t('vms.backup.driveFolder') }}</Label>
            <Input
              id="vm-backup-drive-folder"
              v-model="driveFolder"
              class="font-mono"
              autocomplete="off"
              spellcheck="false"
              :aria-invalid="!folderValid(driveFolder)"
            />
            <p v-if="!folderValid(driveFolder)" class="text-xs text-destructive">
              {{ t('vms.backup.folderInvalid') }}
            </p>
          </div>
        </template>
      </template>

      <p v-if="chosen" :class="hint" data-testid="vm-backup-into">
        {{ t('vms.backup.into', { path: chosen.folder || '/', id: vm.id }) }}
      </p>
      <div role="note" class="rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-xs">
        {{ t('vms.backup.readable') }}
      </div>
      <Alert v-if="isOn(vm.state)" data-testid="vm-backup-running">
        <Info />
        {{ t('vms.backup.running') }}
      </Alert>
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    </form>

    <template #footer>
      <Button type="button" variant="secondary" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button type="submit" form="vm-backup" :disabled="pending || !chosen">
        {{ pending ? t('vms.backup.starting') : t('vms.backup.submit') }}
      </Button>
    </template>
    <FolderPicker v-model:open="browsing" :value="path" writable @pick="path = $event" />
  </ModalDialog>
</template>
