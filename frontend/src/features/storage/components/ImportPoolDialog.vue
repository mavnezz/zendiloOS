<script setup lang="ts">
import { TriangleAlert } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { FoundPool, NamedPool } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { importPool } from '../api'

/**
 * Importing a pool found on the disks: one the configuration names (`known`)
 * as its volume, another adopted with a volume number. A pool another
 * machine used last is imported only once its name is typed.
 */
const props = defineProps<{
  found: FoundPool
  known: NamedPool | null
  freeVolume: number | null
}>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const jobs = useJobsStore()
const confirm = ref('')
const volume = ref<number | undefined>(props.freeVolume ?? undefined)
const error = ref<string | null>(null)
const pending = ref(false)

const adopting = computed(() => props.known === null)
const ready = computed(
  () =>
    !pending.value &&
    (!props.found.other_host || confirm.value.trim() === props.found.name) &&
    (!adopting.value || (volume.value !== undefined && volume.value >= 1 && volume.value <= 999)),
)

watch(open, () => {
  confirm.value = ''
  volume.value = props.freeVolume ?? undefined
  error.value = null
})

async function submit(): Promise<void> {
  pending.value = true
  error.value = null
  try {
    const job = await importPool({
      name: props.found.name,
      id: props.found.id,
      volume: adopting.value ? (volume.value ?? null) : null,
      force: props.found.other_host,
      confirm: props.found.other_host ? confirm.value.trim() : null,
    })
    jobs.update(job)
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
    :title="
      t(adopting ? 'storage.imports.dialog.adoptTitle' : 'storage.imports.dialog.title', {
        name: found.name,
      })
    "
    :description="
      t(adopting ? 'storage.imports.dialog.adoptDescription' : 'storage.imports.dialog.description')
    "
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-4" @submit.prevent="submit">
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <p v-if="known" class="text-sm">
        {{ t('storage.imports.volume', { volume: known.volume, path: known.mount_point }) }}
      </p>
      <div class="grid gap-1 text-xs">
        <p class="font-medium">{{ t('storage.imports.dialog.devices') }}</p>
        <p
          v-for="device in found.devices"
          :key="device.name"
          class="font-mono text-muted-foreground"
        >
          {{ device.name }} · {{ device.state }}
        </p>
      </div>
      <div v-if="adopting" class="grid gap-2">
        <Label for="import-volume">{{ t('storage.imports.dialog.volume') }}</Label>
        <Input
          id="import-volume"
          v-model.number="volume"
          type="number"
          min="1"
          max="999"
          required
        />
      </div>
      <template v-if="found.other_host">
        <Alert variant="destructive">
          <TriangleAlert class="size-4" />
          {{ t('storage.imports.dialog.otherHost') }}
        </Alert>
        <div class="grid gap-2">
          <Label for="import-confirm">{{
            t('storage.imports.dialog.confirm', { name: found.name })
          }}</Label>
          <Input
            id="import-confirm"
            v-model="confirm"
            autocomplete="off"
            spellcheck="false"
            required
          />
        </div>
      </template>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button
          type="submit"
          :variant="found.other_host ? 'destructive' : 'default'"
          :disabled="!ready"
        >
          {{
            t(
              found.other_host
                ? 'storage.imports.dialog.force'
                : adopting
                  ? 'storage.imports.dialog.adoptSubmit'
                  : 'storage.imports.dialog.submit',
            )
          }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
