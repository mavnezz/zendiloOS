<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

import { type AlertVariants, alertVariants } from '.'

const props = defineProps<{ variant?: AlertVariants['variant']; class?: HTMLAttributes['class'] }>()
</script>

<template>
  <div data-slot="alert" role="alert" :class="cn(alertVariants({ variant }), props.class)">
    <slot />
  </div>
</template>
