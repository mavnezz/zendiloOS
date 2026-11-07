<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { ModalDialog } from '@/shared/ui/dialog'

import type { EntryInfo } from '../lib/actions'

/** What is known of an entry, or of the folder shown. */
defineProps<{ entry: EntryInfo | null }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
</script>

<template>
  <ModalDialog
    v-model:open="open"
    :title="t('explorer.properties.title', { name: entry?.name })"
    :close-label="t('desktop.close')"
  >
    <dl v-if="entry" class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-sm">
      <dt class="text-muted-foreground">{{ t('explorer.columns.type') }}</dt>
      <dd>{{ entry.type }}</dd>
      <dt class="text-muted-foreground">{{ t('explorer.properties.location') }}</dt>
      <dd class="break-all">{{ entry.location }}</dd>
      <template v-if="entry.size !== null">
        <dt class="text-muted-foreground">{{ t('files.size') }}</dt>
        <dd>{{ formatBytes(entry.size) }}</dd>
      </template>
      <template v-if="entry.modified !== null">
        <dt class="text-muted-foreground">{{ t('files.modified') }}</dt>
        <dd>{{ formatDateTime(entry.modified * 1000) }}</dd>
      </template>
      <template v-if="entry.access !== undefined">
        <dt class="text-muted-foreground">{{ t('explorer.properties.yourAccess') }}</dt>
        <dd>{{ entry.access ? t(`shares.levels.${entry.access}`) : t('explorer.noAccess') }}</dd>
      </template>
      <template v-if="entry.shared !== undefined">
        <dt class="text-muted-foreground">{{ t('explorer.properties.network') }}</dt>
        <dd>{{ entry.shared ? t('explorer.sharedOn') : t('explorer.sharedOff') }}</dd>
      </template>
    </dl>
  </ModalDialog>
</template>
