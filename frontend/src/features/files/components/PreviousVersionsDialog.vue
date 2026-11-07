<script setup lang="ts">
import { History } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'

import { useShareSnapshots } from '../api'

/** The snapshots of a folder on a volume, to look into; someone who manages shares also takes and removes them. */
const props = defineProps<{ share: number | null; name: string; canManage: boolean }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ browse: [label: string]; manage: [] }>()

const { t } = useI18n()
const { data: snapshots, error } = useShareSnapshots(
  computed(() => (open.value ? props.share : null)),
)
</script>

<template>
  <ModalDialog
    v-model:open="open"
    :title="t('explorer.versions.title', { name })"
    :description="t('explorer.versions.description')"
    :close-label="t('desktop.close')"
  >
    <div class="grid gap-3">
      <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
      <p v-else-if="snapshots && !snapshots.length" class="text-sm text-muted-foreground">
        {{ t('shares.snapshots.none') }}
      </p>
      <ul v-else class="grid max-h-80 gap-1 overflow-y-auto">
        <li
          v-for="snapshot in snapshots"
          :key="snapshot.label"
          class="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-accent/50"
        >
          <History class="size-4 shrink-0 text-muted-foreground" />
          <span class="flex-1 truncate">
            {{ formatDateTime(snapshot.taken_at * 1000) }}
            <span v-if="snapshot.note" class="text-muted-foreground">· {{ snapshot.note }}</span>
          </span>
          <Button
            size="sm"
            variant="outline"
            @click="(emit('browse', snapshot.label), (open = false))"
          >
            {{ t('explorer.versions.browse') }}
          </Button>
        </li>
      </ul>
      <div v-if="canManage" class="flex justify-end">
        <Button variant="outline" size="sm" @click="(emit('manage'), (open = false))">
          {{ t('explorer.versions.manage') }}
        </Button>
      </div>
    </div>
  </ModalDialog>
</template>
