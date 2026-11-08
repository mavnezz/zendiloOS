<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import type { Disk } from '@/shared/api/types'
import { cn } from '@/shared/lib/utils'

defineProps<{
  /** The slot's label, which names the list. */
  label: string
  /** The entry chosen: `none`, `model`, `port:…` or `serial:…`. */
  value: string
  disks: Disk[]
  /** What the model binds here, as the entry that keeps it; none outside a model. */
  modelChoice?: string
  /** A binding no disk present has, kept as an entry so looking loses nothing. */
  absent?: { value: string; label: string } | null
  class?: string
}>()
defineEmits<{ change: [value: string] }>()

const { t } = useI18n()

function describe(disk: Disk): string {
  return t('storage.enclosure.dialog.disk', {
    disk: disk.name,
    model: disk.model ?? t('storage.unknownModel'),
  })
}
</script>

<!-- What a slot holds: nothing, the model's binding, or a disk present by its port or serial number. -->
<template>
  <select
    :value="value"
    :aria-label="label"
    :class="
      cn(
        'h-8 min-w-0 rounded-md border bg-background px-2 text-sm outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 dark:bg-input/30',
        $props.class,
      )
    "
    @change="$emit('change', ($event.target as HTMLSelectElement).value)"
  >
    <option v-if="modelChoice" value="model">{{ modelChoice }}</option>
    <option value="none">{{ t('storage.enclosure.dialog.nothing') }}</option>
    <optgroup :label="t('storage.enclosure.dialog.byPort')">
      <template v-for="disk in disks" :key="disk.name">
        <option v-if="disk.id_path" :value="`port:${disk.id_path}`">{{ describe(disk) }}</option>
      </template>
    </optgroup>
    <optgroup :label="t('storage.enclosure.dialog.bySerial')">
      <template v-for="disk in disks" :key="disk.name">
        <option v-if="disk.serial" :value="`serial:${disk.serial}`">{{ describe(disk) }}</option>
      </template>
    </optgroup>
    <option v-if="absent" :value="absent.value">{{ absent.label }}</option>
  </select>
</template>
