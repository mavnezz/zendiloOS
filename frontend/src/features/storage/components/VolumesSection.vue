<script setup lang="ts">
import { Database } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { formatBytes } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Meter } from '@/shared/ui/meter'

import { useVolumes } from '../api'

const { t } = useI18n()
const { data: volumes, error, isPending } = useVolumes()

const share = (used: number, size: number) => (size > 0 ? used / size : 0)
</script>

<template>
  <div class="grid gap-3">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <p v-else-if="!isPending && !volumes?.length" class="text-sm text-muted-foreground">
      {{ t('storage.noVolumes') }}
    </p>
    <div class="grid gap-3 @3xl:grid-cols-2">
      <section
        v-for="volume in volumes"
        :key="volume.id"
        class="grid gap-3 rounded-xl border bg-card p-4"
      >
        <header class="flex items-start gap-3">
          <Database class="mt-0.5 size-5 shrink-0 text-primary" />
          <div class="min-w-0 flex-1">
            <h3 class="truncate font-medium">{{ volume.label ?? volume.devices.join(', ') }}</h3>
            <p class="truncate text-xs text-muted-foreground">
              {{ volume.filesystem }} ·
              {{
                volume.mount_points.length
                  ? volume.mount_points.join(', ')
                  : t('storage.notMounted')
              }}
            </p>
          </div>
        </header>
        <template v-if="volume.size_bytes != null && volume.used_bytes != null">
          <Meter
            :value="share(volume.used_bytes, volume.size_bytes)"
            :label="t('storage.usedSpace')"
          />
          <p class="text-xs text-muted-foreground">
            {{
              t('storage.spaceUsed', {
                used: formatBytes(volume.used_bytes),
                size: formatBytes(volume.size_bytes),
                available: formatBytes(volume.available_bytes ?? 0),
              })
            }}
          </p>
        </template>
        <p class="text-xs text-muted-foreground">
          {{
            volume.array
              ? t('storage.onArray', { array: volume.array, disks: volume.disks.join(', ') })
              : t('storage.onDisks', { disks: volume.disks.join(', ') })
          }}
        </p>
      </section>
    </div>
  </div>
</template>
