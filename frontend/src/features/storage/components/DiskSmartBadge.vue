<script setup lang="ts">
import { useSmartReport } from '../api'
import SmartBadge from './SmartBadge.vue'

const props = defineProps<{ disk: string }>()

const { data: report, isPending } = useSmartReport(() => props.disk)
</script>

<template>
  <SmartBadge :report="report" :loading="isPending" />
</template>
