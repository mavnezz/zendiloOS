<script setup lang="ts">
import { Download, Eraser, Search, Trash2 } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerImage } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useImages, usePruneImages, usePullImage, useRemoveImage } from '../api'

const { t } = useI18n()
const notifications = useNotifications()
const { data: images, isPending } = useImages()
const pull = usePullImage()
const remove = useRemoveImage()
const prune = usePruneImages()
const filter = ref('')
const pulling = ref(false)
const reference = ref('')
const removing = ref<DockerImage | null>(null)
const pruning = ref(false)

const shown = computed(() => {
  const term = filter.value.trim().toLowerCase()
  return (images.value ?? []).filter(
    (i) => !term || i.references.some((r) => r.toLowerCase().includes(term)),
  )
})
const error = computed(() => remove.error.value ?? prune.error.value)
const short = (id: string) => id.replace('sha256:', '').slice(0, 12)

function fetch(): void {
  const value = reference.value.trim()
  if (!value) return
  pull.mutate(value, {
    onSuccess: () => {
      pulling.value = false
      reference.value = ''
    },
  })
}

function clean(): void {
  prune.mutate(undefined, {
    onSuccess: (result) =>
      notifications.notify('success', t('docker.images.pruned'), result.text.trim()),
  })
}
function confirmRemove(): void {
  if (removing.value) remove.mutate(removing.value.id)
  removing.value = null
}
</script>

<!-- The images on the engine and which containers they made. -->
<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <Button @click="pulling = true">
        <Download />
        {{ t('docker.images.pull') }}
      </Button>
      <Button variant="secondary" :disabled="prune.isPending.value" @click="pruning = true">
        <Eraser />
        {{ t('docker.images.prune') }}
      </Button>
      <div class="relative ml-auto w-full max-w-60">
        <Search class="absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          v-model="filter"
          class="pl-8"
          :placeholder="t('docker.search')"
          :aria-label="t('docker.search')"
        />
      </div>
    </div>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <div v-if="isPending" class="h-40 animate-pulse rounded-xl bg-muted" />
    <div v-else class="overflow-hidden rounded-xl border bg-card">
      <Table>
        <TableHeader class="bg-muted/50">
          <TableRow class="hover:bg-transparent">
            <TableHead>{{ t('docker.images.name') }}</TableHead>
            <TableHead>ID</TableHead>
            <TableHead>{{ t('docker.images.size') }}</TableHead>
            <TableHead class="hidden @3xl:table-cell">{{ t('docker.images.created') }}</TableHead>
            <TableHead>{{ t('docker.images.usedBy') }}</TableHead>
            <TableHead class="w-12"
              ><span class="sr-only">{{ t('people.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="image in shown" :key="image.id">
            <TableCell>
              <span class="grid gap-0.5">
                <span v-for="name in image.references" :key="name" class="font-medium">{{
                  name
                }}</span>
                <span v-if="!image.references.length" class="text-muted-foreground">{{
                  t('docker.images.unnamed')
                }}</span>
              </span>
            </TableCell>
            <TableCell class="font-mono text-[13px] text-muted-foreground">{{
              short(image.id)
            }}</TableCell>
            <TableCell class="tabular-nums">{{ formatBytes(image.size_bytes) }}</TableCell>
            <TableCell class="hidden text-muted-foreground @3xl:table-cell">{{
              image.created_at ? formatDateTime(image.created_at * 1000) : '–'
            }}</TableCell>
            <TableCell>
              <span v-if="image.used_by.length" class="text-sm">{{
                image.used_by.join(', ')
              }}</span>
              <Badge v-else variant="outline">{{ t('docker.images.unused') }}</Badge>
            </TableCell>
            <TableCell>
              <Button
                variant="ghost"
                size="icon"
                :disabled="image.used_by.length > 0"
                :title="t('docker.actions.delete')"
                :aria-label="t('docker.actions.delete')"
                @click="removing = image"
              >
                <Trash2 />
              </Button>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
    <ModalDialog
      v-model:open="pulling"
      :title="t('docker.images.pull')"
      :close-label="t('desktop.close')"
    >
      <form class="grid gap-3" @submit.prevent="fetch">
        <Input
          v-model="reference"
          class="font-mono text-[13px]"
          placeholder="postgres:17"
          :aria-label="t('docker.images.reference')"
        />
        <p class="text-xs text-muted-foreground">{{ t('docker.images.pullHint') }}</p>
        <Alert v-if="pull.error.value" variant="destructive">{{
          problemMessage(pull.error.value)
        }}</Alert>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="ghost" @click="pulling = false">{{
            t('desktop.cancel')
          }}</Button>
          <Button type="submit" :disabled="!reference.trim() || pull.isPending.value">{{
            t('docker.images.pullNow')
          }}</Button>
        </div>
      </form>
    </ModalDialog>
    <ConfirmDialog
      :open="removing !== null"
      :title="
        t('docker.images.removeTitle', {
          name: removing?.references[0] ?? short(removing?.id ?? ''),
        })
      "
      :description="t('docker.images.removeText')"
      :confirm-label="t('docker.actions.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open: boolean) => !open && (removing = null)"
      @confirm="confirmRemove"
    />
    <ConfirmDialog
      v-model:open="pruning"
      :title="t('docker.images.prune')"
      :description="t('docker.images.pruneText')"
      :confirm-label="t('docker.images.prune')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @confirm="clean"
    />
  </div>
</template>
