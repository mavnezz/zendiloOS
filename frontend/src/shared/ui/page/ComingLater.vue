<script setup lang="ts">
import type { Component } from 'vue'
import { useI18n } from 'vue-i18n'

defineProps<{ icon: Component; title: string; description: string }>()
const { t } = useI18n()
</script>

<!-- A page that is planned: what it will be for. -->
<template>
  <div class="flex min-h-full flex-col items-center justify-center gap-3 py-12 text-center">
    <div class="flex size-12 items-center justify-center rounded-xl bg-muted text-muted-foreground">
      <component :is="icon" class="size-6" />
    </div>
    <div class="space-y-1">
      <h2 class="text-lg font-semibold">{{ title }}</h2>
      <p class="max-w-sm text-sm text-muted-foreground">{{ description }}</p>
    </div>
    <span class="rounded-full border px-2.5 py-0.5 text-xs text-muted-foreground">{{
      t('common.comingLater')
    }}</span>
  </div>
</template>
