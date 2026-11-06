<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{ class?: HTMLAttributes['class'] }>()
</script>

<template>
  <td :class="cn('h-10 border-b px-3 align-middle', props.class)"><slot /></td>
</template>
