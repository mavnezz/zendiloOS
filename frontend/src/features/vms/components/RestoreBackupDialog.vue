<script setup lang="ts">
import { TriangleAlert } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine, VmBackup, VmPlugin } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'

import { useBackupActions } from '../api'
import { NAME_MAX } from '../lib/vms'

/**
 * Restores a backup as a new VM, or over `vm`, the VM it came from, while
 * that is shut off. `taken`: the names of the VMs there are.
 */
const props = defineProps<{
  backup: VmBackup
  plugin: VmPlugin
  vm: VirtualMachine | null
  taken: string[]
}>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const jobs = useJobsStore()
const actions = useBackupActions()

const how = ref<'new' | 'over'>('new')
const name = ref('')
const pool = ref('')
const confirm = ref('')
const pending = ref(false)
const failure = ref<string | null>(null)

watch(
  open,
  (isOpen) => {
    if (!isOpen) return
    how.value = 'new'
    name.value = t('vms.backups.restoreDialog.nameSuffix', { name: props.backup.vm_name })
    pool.value = ''
    confirm.value = ''
    failure.value = null
  },
  { immediate: true },
)

const when = computed(() => formatDateTime(props.backup.taken_at * 1000))
const off = computed(() => props.vm?.state === 'shut_off')
const nameValid = computed(() => {
  const trimmed = name.value.trim()
  return (
    trimmed !== '' &&
    [...trimmed].length <= NAME_MAX &&
    !props.taken.some((other) => other.toLowerCase() === trimmed.toLowerCase())
  )
})
const ready = computed(() =>
  how.value === 'new'
    ? nameValid.value
    : props.vm !== null && off.value && confirm.value.trim() === props.vm.name,
)

async function restore(): Promise<void> {
  if (!ready.value) return
  pending.value = true
  failure.value = null
  try {
    const restored = await actions.restore(
      props.backup.id,
      how.value === 'new'
        ? { kind: 'new', name: name.value.trim(), pool: pool.value || null }
        : { kind: 'over', confirm: confirm.value.trim() },
    )
    jobs.update(restored.job)
    open.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    pending.value = false
  }
}

const hint = 'text-xs text-muted-foreground'
</script>

<!-- A backup restored: as a new VM, or over the one it came from. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('vms.backups.restoreDialog.title', { name: backup.vm_name, when })"
    :close-label="t('desktop.close')"
  >
    <form id="vm-restore" class="grid gap-4" @submit.prevent="restore">
      <div role="radiogroup" class="grid gap-2 text-sm">
        <label class="flex items-start gap-2 rounded-lg border p-3">
          <input
            v-model="how"
            type="radio"
            name="vm-restore-how"
            value="new"
            class="mt-1 accent-primary"
          />
          <span class="grid gap-0.5">
            <span class="font-medium">{{ t('vms.backups.restoreDialog.asNew') }}</span>
            <span :class="hint">{{ t('vms.backups.restoreDialog.asNewHint') }}</span>
          </span>
        </label>
        <label class="flex items-start gap-2 rounded-lg border p-3" :class="vm ? '' : 'opacity-60'">
          <input
            v-model="how"
            type="radio"
            name="vm-restore-how"
            value="over"
            class="mt-1 accent-primary"
            :disabled="!vm"
          />
          <span class="grid gap-0.5">
            <span class="font-medium">{{
              t('vms.backups.restoreDialog.over', { name: vm?.name ?? backup.vm_name })
            }}</span>
            <span :class="hint">{{
              vm ? t('vms.backups.restoreDialog.overHint') : t('vms.backups.restoreDialog.overGone')
            }}</span>
          </span>
        </label>
      </div>

      <template v-if="how === 'new'">
        <div class="grid gap-2">
          <Label for="vm-restore-name">{{ t('vms.backups.restoreDialog.name') }}</Label>
          <Input
            id="vm-restore-name"
            v-model="name"
            maxlength="48"
            autocomplete="off"
            :aria-invalid="!nameValid"
          />
          <p v-if="!nameValid" class="text-xs text-destructive">
            {{ name.trim() ? t('vms.wizard.invalid.nameTaken') : t('vms.wizard.invalid.name') }}
          </p>
        </div>
        <div v-if="plugin.locations.length > 1" class="grid gap-2">
          <Label for="vm-restore-pool">{{ t('vms.backups.restoreDialog.location') }}</Label>
          <NativeSelect id="vm-restore-pool" v-model="pool">
            <option value="">{{ t('vms.backups.restoreDialog.first') }}</option>
            <option
              v-for="location in plugin.locations"
              :key="location.pool"
              :value="location.pool"
            >
              {{ t('vms.storage.pool', { pool: location.pool }) }}
            </option>
          </NativeSelect>
        </div>
      </template>
      <template v-else-if="vm">
        <Alert v-if="!off" variant="destructive" data-testid="vm-restore-running">
          {{ t('vms.backups.restoreDialog.overRunning', { name: vm.name }) }}
        </Alert>
        <div class="grid gap-2 rounded-lg border border-destructive/40 bg-destructive/5 p-3">
          <p class="flex items-start gap-2 text-sm text-destructive">
            <TriangleAlert class="mt-0.5 size-4 shrink-0" />
            {{ t('vms.backups.restoreDialog.overWarning') }}
          </p>
          <Label for="vm-restore-confirm">{{
            t('vms.backups.restoreDialog.confirm', { name: vm.name })
          }}</Label>
          <Input
            id="vm-restore-confirm"
            v-model="confirm"
            class="max-w-xs"
            autocomplete="off"
            spellcheck="false"
          />
        </div>
      </template>
      <p :class="hint">{{ t('vms.backups.restoreDialog.notRestored') }}</p>
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    </form>

    <template #footer>
      <Button type="button" variant="secondary" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button
        type="submit"
        form="vm-restore"
        :variant="how === 'over' ? 'destructive' : 'default'"
        :disabled="pending || !ready"
      >
        {{ t('vms.backups.restoreDialog.submit') }}
      </Button>
    </template>
  </ModalDialog>
</template>
