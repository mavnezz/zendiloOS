<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{ id?: string; disabled?: boolean; class?: HTMLAttributes['class'] }>()
const value = defineModel<string | number>({ required: true })
</script>

<!--
  The browser's own list: fast with hundreds of entries (time zones), typed
  into to jump, and native on phones. Its arrow is the one every select
  gets (global.css).
-->
<template>
  <div :class="cn('relative', props.class)">
    <select
      :id="id"
      v-model="value"
      :disabled="disabled"
      data-slot="native-select"
      class="h-8 w-full appearance-none rounded-md border bg-background py-1 pr-8 pl-3 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 dark:bg-input/30"
    >
      <slot />
    </select>
  </div>
</template>
