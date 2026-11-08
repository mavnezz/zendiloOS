<script setup lang="ts">
import { LoaderCircle } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { ModalDialog } from '@/shared/ui/dialog'

import { useEntryFacts } from '../api'
import type { EntryInfo } from '../lib/actions'

/** What is known of an entry, or of the folder shown; the NAS is asked for the rest. */
const props = defineProps<{ entry: EntryInfo | null }>()
const open = defineModel<boolean>('open', { default: false })

const { t, locale } = useI18n()
/** A count as the interface's language writes it. */
const count = (value: number) => value.toLocaleString(locale.value)
/** Counted while the dialog is open; closing it calls the count off. */
const { facts, counting } = useEntryFacts(() => (open.value ? (props.entry?.at ?? null) : null))
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
      <template v-if="facts?.folder || (counting && entry.folder)">
        <dt class="text-muted-foreground">{{ t('explorer.properties.contains') }}</dt>
        <dd role="status">
          <span v-if="counting" class="flex items-center gap-2 text-muted-foreground">
            <LoaderCircle class="size-4 animate-spin" />
            {{ t('explorer.properties.counting') }}
          </span>
          <template v-else-if="facts">
            {{
              t('explorer.properties.counted', {
                files: count(facts.files),
                folders: count(facts.folders),
                hidden: count(facts.hidden),
                bytes: formatBytes(facts.bytes),
              })
            }}
            <span v-if="facts.stopped" class="block text-xs text-muted-foreground">
              {{ t(`explorer.properties.stopped.${facts.stopped}`) }}
            </span>
          </template>
        </dd>
      </template>
      <template v-else-if="(facts?.bytes ?? entry.size) !== null">
        <dt class="text-muted-foreground">{{ t('files.size') }}</dt>
        <dd>{{ formatBytes(facts?.bytes ?? entry.size ?? 0) }}</dd>
      </template>
      <template v-if="(facts?.modified ?? entry.modified) !== null">
        <dt class="text-muted-foreground">{{ t('files.modified') }}</dt>
        <dd>{{ formatDateTime((facts?.modified ?? entry.modified ?? 0) * 1000) }}</dd>
      </template>
      <template v-if="facts?.created">
        <dt class="text-muted-foreground">{{ t('explorer.properties.created') }}</dt>
        <dd>{{ formatDateTime(facts.created * 1000) }}</dd>
      </template>
      <template v-if="facts?.owner">
        <dt class="text-muted-foreground">{{ t('explorer.properties.owner') }}</dt>
        <dd>{{ facts.owner }}</dd>
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
