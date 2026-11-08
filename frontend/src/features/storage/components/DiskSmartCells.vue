<script setup lang="ts">
import { useSmartReport } from '../api'
import SmartBadge from './SmartBadge.vue'

const props = defineProps<{ disk: string }>()
const { data: report, isPending } = useSmartReport(() => props.disk)

/** Hours powered on as days or years, for a quick read. */
function age(hours: number): string {
  const days = hours / 24
  return days >= 365 ? `${(days / 365).toFixed(1)} y` : `${Math.round(days)} d`
}
</script>

<!-- Temperature, power-on time and the SMART verdict: three cells of a disk's row. -->
<template>
  <td class="h-10 border-b px-3 text-right align-middle whitespace-nowrap tabular-nums">
    <span
      v-if="report?.temperature_celsius != null"
      :class="report.temperature_celsius >= 55 ? 'font-medium text-destructive' : ''"
    >
      {{ report.temperature_celsius }} °C
    </span>
    <span v-else class="text-muted-foreground">–</span>
  </td>
  <td
    class="h-10 border-b px-3 text-right align-middle whitespace-nowrap text-muted-foreground tabular-nums"
  >
    {{ report?.power_on_hours != null ? age(report.power_on_hours) : '–' }}
  </td>
  <td class="h-10 border-b px-3 align-middle">
    <SmartBadge :report="report" :loading="isPending" />
  </td>
</template>
