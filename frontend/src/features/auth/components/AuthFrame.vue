<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import { BrandMark } from '@/shared/ui/brand'

import { deviceName } from '../device'

const { t } = useI18n()
</script>

<!--
  Common frame of the setup and sign-in pages. Under the product's name it
  says which NAS this is, where the NAS tells (its own network): with
  several of them, the name is what tells their sign-in pages apart.
-->
<template>
  <div class="wallpaper flex min-h-dvh flex-col items-center justify-center gap-6 p-6">
    <div class="flex items-center gap-2">
      <BrandMark class="size-7" />
      <span
        class="text-lg font-semibold tracking-tight text-white [text-shadow:0_1px_3px_rgb(0_0_0/0.5)] in-data-[wallpaper-tone=light]:text-neutral-900 in-data-[wallpaper-tone=light]:[text-shadow:none]"
      >
        {{ t('app.name') }}
      </span>
    </div>
    <h1
      v-if="deviceName"
      data-testid="device-name"
      class="display -mt-3 max-w-full truncate text-4xl text-white [text-shadow:0_1px_3px_rgb(0_0_0/0.5)] in-data-[wallpaper-tone=light]:text-neutral-900 in-data-[wallpaper-tone=light]:[text-shadow:none]"
    >
      {{ deviceName }}
    </h1>
    <div class="w-full max-w-sm">
      <slot />
    </div>
  </div>
</template>
