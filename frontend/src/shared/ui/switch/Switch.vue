<script setup lang="ts">
import { SwitchRoot, SwitchThumb } from 'reka-ui'
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{ id?: string; disabled?: boolean; class?: HTMLAttributes['class'] }>()
const checked = defineModel<boolean>({ default: false })
</script>

<!-- On or off, for a setting that takes effect as a whole. -->
<template>
  <SwitchRoot
    :id="id"
    v-model="checked"
    :disabled="disabled"
    data-slot="switch"
    :class="
      cn(
        'peer relative inline-flex h-[18.4px] w-8 shrink-0 items-center rounded-full border border-transparent transition-all outline-none after:absolute after:-inset-x-3 after:-inset-y-2 focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:bg-primary data-[state=unchecked]:bg-input dark:data-[state=unchecked]:bg-input/80',
        props.class,
      )
    "
  >
    <SwitchThumb
      class="pointer-events-none block size-4 rounded-full bg-background ring-0 shadow-sm transition-transform data-[state=checked]:translate-x-[calc(100%-2px)] data-[state=unchecked]:translate-x-0 dark:data-[state=checked]:bg-primary-foreground dark:data-[state=unchecked]:bg-foreground"
    />
  </SwitchRoot>
</template>
