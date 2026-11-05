<script setup lang="ts">
import { LoaderCircle, Power } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import { usePower } from './power'

const { t } = useI18n()
const power = usePower()
</script>

<!-- Covers the desktop while the system restarts or after it was shut down. -->
<template>
  <div
    v-if="power.phase !== 'running'"
    class="wallpaper fixed inset-0 z-[2000] grid place-items-center p-6"
    role="status"
  >
    <div class="flex max-w-sm flex-col items-center gap-3 text-center">
      <LoaderCircle v-if="power.phase === 'restarting'" class="size-10 animate-spin text-primary" />
      <Power v-else class="size-10 text-muted-foreground" />
      <h1 class="text-xl font-semibold">
        {{ power.phase === 'restarting' ? t('desktop.restarting') : t('desktop.poweredOff') }}
      </h1>
      <p class="text-sm text-muted-foreground">
        {{
          power.phase === 'restarting' ? t('desktop.restartingHint') : t('desktop.poweredOffHint')
        }}
      </p>
    </div>
  </div>
</template>
