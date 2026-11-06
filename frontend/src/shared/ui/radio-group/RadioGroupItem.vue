<script setup lang="ts">
import { RadioGroupIndicator, RadioGroupItem } from 'reka-ui'

defineProps<{ value: string; id?: string; disabled?: boolean }>()
</script>

<!-- One choice of a radio group, with its label beside it. -->
<template>
  <label
    class="flex w-fit items-center gap-2 text-sm has-[[data-disabled]]:cursor-not-allowed has-[[data-disabled]]:opacity-50"
  >
    <RadioGroupItem
      :id="id"
      :value="value"
      :disabled="disabled"
      class="relative flex aspect-square size-4 shrink-0 items-center justify-center rounded-full border border-input shadow-xs outline-none transition focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 data-[state=checked]:border-primary dark:bg-input/30"
    >
      <RadioGroupIndicator class="size-2 rounded-full bg-primary" />
    </RadioGroupItem>
    <slot />
  </label>
</template>
