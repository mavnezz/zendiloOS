<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import { BrandMark } from '@/shared/ui/brand'

const { t } = useI18n()
</script>

<!-- Common frame of the setup and sign-in pages. -->
<template>
  <div class="wallpaper flex min-h-dvh flex-col items-center justify-center gap-8 p-6">
    <div class="flex items-center gap-3">
      <BrandMark class="size-10" />
      <span class="text-2xl font-semibold tracking-tight">{{ t('app.name') }}</span>
    </div>
    <div class="w-full max-w-sm">
      <slot />
    </div>
  </div>
</template>
