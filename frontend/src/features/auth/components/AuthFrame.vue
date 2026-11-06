<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import { BrandMark } from '@/shared/ui/brand'

const { t } = useI18n()
</script>

<!-- Common frame of the setup and sign-in pages. -->
<template>
  <div class="wallpaper flex min-h-dvh flex-col items-center justify-center gap-6 p-6">
    <div class="flex items-center gap-2">
      <BrandMark class="size-7" />
      <span
        class="text-lg font-semibold tracking-tight text-white [text-shadow:0_1px_3px_rgb(0_0_0/0.5)] in-data-[wallpaper-tone=light]:text-foreground in-data-[wallpaper-tone=light]:[text-shadow:none]"
      >
        {{ t('app.name') }}
      </span>
    </div>
    <div class="w-full max-w-sm">
      <slot />
    </div>
  </div>
</template>
