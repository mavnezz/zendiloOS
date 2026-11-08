<script setup lang="ts">
import { computed } from 'vue'
import { encode } from 'uqr'

const props = defineProps<{ value: string; label: string }>()

/** The dark modules as one path, a unit square each; with the quiet zone around. */
const code = computed(() => {
  const { data, size } = encode(props.value, { ecc: 'M', border: 4 })
  let path = ''
  data.forEach((row, y) =>
    row.forEach((dark, x) => {
      if (dark) path += `M${x} ${y}h1v1h-1z`
    }),
  )
  return { path, size }
})
</script>

<!-- A QR code, dark on white in either theme, as scanners expect. -->
<template>
  <svg
    role="img"
    :aria-label="label"
    :viewBox="`0 0 ${code.size} ${code.size}`"
    shape-rendering="crispEdges"
    class="rounded-lg bg-white text-black"
  >
    <path :d="code.path" fill="currentColor" />
  </svg>
</template>
