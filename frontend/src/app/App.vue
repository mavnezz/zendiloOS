<script setup lang="ts">
import { TriangleAlert } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import { staleBuild } from '@/shared/lib/stale-build'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'

const { t } = useI18n()

function reload(): void {
  window.location.reload()
}
</script>

<template>
  <!-- Parts of the interface did not load although the page was reloaded (stale-build.ts). -->
  <Alert
    v-if="staleBuild"
    variant="destructive"
    class="fixed inset-x-4 top-4 z-[2100] mx-auto max-w-xl bg-card shadow-lg"
  >
    <TriangleAlert />
    <span class="flex flex-wrap items-center gap-3">
      {{ t('app.staleBuild') }}
      <Button size="sm" variant="outline" @click="reload">{{ t('app.reload') }}</Button>
    </span>
  </Alert>
  <RouterView />
</template>
