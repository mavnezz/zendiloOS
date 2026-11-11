<script setup lang="ts">
import { ArrowDown, ArrowUp, Disc3, HardDrive, Plus, Trash2 } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VmImage } from '@/shared/api/types'
import { Button } from '@/shared/ui/button'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'
import { Switch } from '@/shared/ui/switch'

import {
  DRIVES_MAX,
  addDrive,
  bootChoices,
  moveBoot,
  removeDrive,
  toggleBoot,
  type BootEntry,
  type VmForm,
} from '../lib/vms'

/**
 * A VM's CD drives with the library's images in them, and the order it
 * boots in. `fixed`: the VM runs, so the drives and the boot order stay as
 * they are; only the images in them change.
 */
defineProps<{ images: VmImage[]; fixed?: boolean }>()
const form = defineModel<VmForm>('form', { required: true })
const emit = defineEmits<{ library: [] }>()

const { t } = useI18n()

function setImage(slot: number, value: string): void {
  const image = value === '' ? null : value
  form.value = {
    ...form.value,
    drives: form.value.drives.map((drive) => (drive.slot === slot ? { ...drive, image } : drive)),
  }
}

function device(entry: BootEntry): string {
  return entry.kind === 'disk'
    ? t('vms.devices.disk', { n: entry.slot + 1 })
    : t('vms.devices.drive', { n: entry.slot + 1 })
}

const order = computed(() => bootChoices(form.value))
</script>

<!-- CD drives and what is in them; then the boot order. -->
<template>
  <section class="grid gap-3" aria-labelledby="vm-media-heading">
    <h4 id="vm-media-heading" class="text-sm font-medium">{{ t('vms.wizard.media') }}</h4>
    <p v-if="!form.drives.length" class="text-sm text-muted-foreground">
      {{ t('vms.devices.noDrives') }}
    </p>
    <ul class="grid gap-2">
      <li
        v-for="drive in form.drives"
        :key="drive.slot"
        :data-drive="drive.slot"
        class="grid gap-3 rounded-lg border p-3 sm:grid-cols-[10rem_1fr_auto] sm:items-end"
      >
        <span class="flex items-center gap-2 text-sm font-medium sm:self-center">
          <Disc3 class="size-4 text-muted-foreground" />
          {{ t('vms.devices.drive', { n: drive.slot + 1 }) }}
        </span>
        <div class="grid min-w-0 gap-1.5">
          <Label :for="`vm-drive-${drive.slot}`" class="text-xs">{{
            t('vms.devices.image')
          }}</Label>
          <NativeSelect
            :id="`vm-drive-${drive.slot}`"
            :model-value="drive.image ?? ''"
            @update:model-value="(value) => setImage(drive.slot, String(value))"
          >
            <option value="">{{ t('vms.devices.noImage') }}</option>
            <option
              v-for="image in images"
              :key="image.id"
              :value="image.id"
              :disabled="!image.ready"
            >
              {{ image.name }}
            </option>
          </NativeSelect>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          :aria-label="t('vms.devices.removeDrive', { n: drive.slot + 1 })"
          :disabled="fixed"
          @click="form = removeDrive(form, drive.slot)"
        >
          <Trash2 />
        </Button>
      </li>
    </ul>
    <div class="flex flex-wrap items-center gap-2">
      <Button
        type="button"
        size="sm"
        variant="outline"
        :disabled="fixed || form.drives.length >= DRIVES_MAX"
        @click="form = addDrive(form)"
      >
        <Plus />
        {{ t('vms.devices.addDrive') }}
      </Button>
      <template v-if="!images.length">
        <span class="text-xs text-muted-foreground">{{ t('vms.devices.noImages') }}</span>
        <Button type="button" size="sm" variant="link" @click="emit('library')">
          {{ t('vms.devices.toImages') }}
        </Button>
      </template>
    </div>

    <h4 class="mt-2 text-sm font-medium">{{ t('vms.devices.boot') }}</h4>
    <ol class="grid gap-1.5" data-testid="vm-boot-order">
      <li
        v-for="({ entry, on }, index) in order"
        :key="`${entry.kind}${entry.slot}`"
        :data-boot="`${entry.kind}:${entry.slot}`"
        class="flex items-center gap-3 rounded-lg border px-3 py-2 text-sm"
        :class="!on && 'text-muted-foreground'"
      >
        <Switch
          :id="`vm-boot-${entry.kind}-${entry.slot}`"
          :model-value="on"
          :disabled="fixed"
          :aria-label="t('vms.devices.bootFrom', { device: device(entry) })"
          @update:model-value="form = toggleBoot(form, entry)"
        />
        <component :is="entry.kind === 'disk' ? HardDrive : Disc3" class="size-4" />
        <span class="flex-1">{{ device(entry) }}</span>
        <span v-if="on" class="text-xs text-muted-foreground tabular-nums">{{ index + 1 }}</span>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          :aria-label="t('vms.devices.earlier', { device: device(entry) })"
          :disabled="fixed || !on || index === 0"
          @click="form = moveBoot(form, entry, -1)"
        >
          <ArrowUp />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          :aria-label="t('vms.devices.later', { device: device(entry) })"
          :disabled="fixed || !on || index === form.boot.length - 1"
          @click="form = moveBoot(form, entry, 1)"
        >
          <ArrowDown />
        </Button>
      </li>
    </ol>
    <p class="text-xs text-muted-foreground">{{ t('vms.devices.bootHint') }}</p>
  </section>
</template>
