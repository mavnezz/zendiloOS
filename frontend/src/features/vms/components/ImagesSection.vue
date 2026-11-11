<script setup lang="ts">
import { Disc3, FolderInput, LoaderCircle, Pencil, Trash2, Upload } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VmImage, VmPlugin } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { ActionMenu, type Action } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useImageActions, useVmImages } from '../api'
import { imageNameValid } from '../lib/vms'
import AddImageDialog from './AddImageDialog.vue'
import UploadImageDialog from './UploadImageDialog.vue'

/** The image library: ISOs kept once, for every VM that wants one. */
const props = defineProps<{ plugin: VmPlugin }>()

const { t } = useI18n()
const actions = useImageActions()
const { data: images, error, isPending } = useVmImages()

const adding = ref(false)
const uploading = ref(false)
const renaming = ref<VmImage | null>(null)
const newName = ref('')
const deleting = ref<VmImage | null>(null)
const failure = ref<string | null>(null)

watch(renaming, (image) => (newName.value = image?.name ?? ''))

const menu = (image: VmImage): Action[] => [
  { id: 'rename', label: t('vms.images.rename'), icon: Pencil, disabled: !image.ready },
  {
    id: 'delete',
    label: t('vms.images.delete'),
    icon: Trash2,
    destructive: true,
    disabled: !image.ready,
  },
]

function select(image: VmImage, id: string): void {
  failure.value = null
  if (id === 'rename') renaming.value = image
  else deleting.value = image
}

async function rename(): Promise<void> {
  const image = renaming.value
  if (!image || !imageNameValid(newName.value)) return
  try {
    await actions.rename(image.id, newName.value.trim())
    renaming.value = null
  } catch (problem) {
    failure.value = problemMessage(problem)
    renaming.value = null
  }
}

async function remove(): Promise<void> {
  const image = deleting.value
  if (!image) return
  try {
    await actions.remove(image.id)
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}

function origin(image: VmImage): string {
  return image.origin.kind === 'share'
    ? `${image.origin.share}/${image.origin.path}`
    : t('vms.images.uploaded', { file: image.origin.file })
}

const max = computed(() => props.plugin.image_upload_max_bytes)
</script>

<template>
  <div class="grid content-start gap-6">
    <header class="flex flex-wrap items-center gap-3">
      <div class="grid min-w-0 flex-1 gap-0.5">
        <h2 class="text-xl font-semibold">{{ t('vms.sections.images') }}</h2>
        <p class="text-sm text-muted-foreground">{{ t('vms.images.description') }}</p>
      </div>
      <Button variant="outline" @click="adding = true">
        <FolderInput />
        {{ t('vms.images.fromNas') }}
      </Button>
      <Button @click="uploading = true">
        <Upload />
        {{ t('vms.images.upload') }}
      </Button>
    </header>

    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <Alert v-if="failure" variant="destructive" data-testid="vm-images-failure">{{
      failure
    }}</Alert>

    <div v-if="isPending" class="h-32 animate-pulse rounded-xl bg-muted" />
    <div
      v-else-if="!images?.length"
      class="grid justify-items-center gap-3 rounded-xl border border-dashed p-10 text-center"
      data-testid="vm-images-empty"
    >
      <Disc3 class="size-10 text-muted-foreground" />
      <p class="font-medium">{{ t('vms.images.empty') }}</p>
      <p class="max-w-md text-sm text-muted-foreground">{{ t('vms.images.emptyHint') }}</p>
    </div>
    <ul v-else class="grid gap-2">
      <li
        v-for="image in images"
        :key="image.id"
        :data-image="image.id"
        class="flex flex-wrap items-center gap-3 rounded-xl border bg-card p-3"
      >
        <span
          class="grid size-9 shrink-0 place-items-center rounded-lg bg-muted text-muted-foreground"
          aria-hidden="true"
        >
          <Disc3 class="size-5" />
        </span>
        <div class="grid min-w-0 flex-1 gap-0.5">
          <span class="truncate font-medium" :title="image.name">{{ image.name }}</span>
          <span class="truncate text-xs text-muted-foreground" :title="origin(image)">
            {{ origin(image) }} · {{ formatDateTime(image.added_at * 1000) }}
          </span>
        </div>
        <span class="text-sm tabular-nums">{{
          image.bytes !== null && image.bytes !== undefined ? formatBytes(image.bytes) : '–'
        }}</span>
        <Badge v-if="!image.ready" variant="info">
          <LoaderCircle class="animate-spin" />
          {{ t('vms.images.adding') }}
        </Badge>
        <span v-else class="flex max-w-60 flex-wrap gap-1" :aria-label="t('vms.images.usedBy')">
          <Badge v-for="vm in image.used_by" :key="vm" variant="secondary">{{ vm }}</Badge>
          <span v-if="!image.used_by.length" class="text-xs text-muted-foreground">{{
            t('vms.images.unused')
          }}</span>
        </span>
        <ActionMenu
          :actions="menu(image)"
          :label="t('vms.images.more', { name: image.name })"
          @select="(id: string) => select(image, id)"
        />
      </li>
    </ul>

    <AddImageDialog v-model:open="adding" />
    <UploadImageDialog v-model:open="uploading" :max="max" />
    <ModalDialog
      :open="Boolean(renaming)"
      :title="t('vms.images.renameDialog.title', { name: renaming?.name ?? '' })"
      :close-label="t('desktop.close')"
      @update:open="(open: boolean) => !open && (renaming = null)"
    >
      <form class="grid gap-4" @submit.prevent="rename">
        <div class="grid gap-2">
          <Label for="vm-image-rename">{{ t('vms.images.name') }}</Label>
          <Input
            id="vm-image-rename"
            v-model="newName"
            maxlength="128"
            autocomplete="off"
            :aria-invalid="!imageNameValid(newName)"
          />
        </div>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="outline" @click="renaming = null">
            {{ t('desktop.cancel') }}
          </Button>
          <Button type="submit" :disabled="!imageNameValid(newName)">
            {{ t('vms.images.renameDialog.submit') }}
          </Button>
        </div>
      </form>
    </ModalDialog>
    <ConfirmDialog
      :open="Boolean(deleting)"
      :title="t('vms.images.deleteTitle', { name: deleting?.name ?? '' })"
      :description="t('vms.images.deleteText')"
      :confirm-label="t('vms.images.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open: boolean) => !open && (deleting = null)"
      @confirm="remove"
    />
  </div>
</template>
