<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{ class?: HTMLAttributes['class'] }>()
</script>

<!-- Three drive bays, as on the front of a NAS. -->
<template>
  <svg viewBox="0 0 32 32" aria-hidden="true" :class="cn('size-8 text-primary', props.class)">
    <rect width="32" height="32" rx="8" fill="currentColor" />
    <rect x="7" y="8" width="18" height="4" rx="2" fill="white" fill-opacity="0.95" />
    <rect x="7" y="14" width="18" height="4" rx="2" fill="white" fill-opacity="0.75" />
    <rect x="7" y="20" width="18" height="4" rx="2" fill="white" fill-opacity="0.55" />
    <circle cx="22" cy="10" r="1.2" fill="currentColor" />
  </svg>
</template>
