<script setup lang="ts">
import { RadioGroupRoot } from 'reka-ui'
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{ disabled?: boolean; label?: string; class?: HTMLAttributes['class'] }>()
const value = defineModel<string>({ required: true })
</script>

<template>
  <RadioGroupRoot
    v-model="value"
    :disabled="disabled"
    :aria-label="label"
    data-slot="radio-group"
    :class="cn('grid gap-2', props.class)"
  >
    <slot />
  </RadioGroupRoot>
</template>
