<script setup lang="ts">
import { computed, ref } from 'vue'

defineProps<{ label: string; readonly?: boolean; minRows?: number }>()
const text = defineModel<string>({ required: true })

const area = ref<HTMLTextAreaElement | null>(null)
const gutter = ref<HTMLElement | null>(null)
const lines = computed(() => Math.max(1, text.value.split('\n').length))

function follow(): void {
  if (area.value && gutter.value) gutter.value.scrollTop = area.value.scrollTop
}

/** Tab indents by two spaces, as YAML wants, instead of leaving the field. */
function indent(event: KeyboardEvent): void {
  const field = area.value
  if (!field || event.shiftKey) return
  event.preventDefault()
  const { selectionStart: start, selectionEnd: end } = field
  text.value = `${text.value.slice(0, start)}  ${text.value.slice(end)}`
  requestAnimationFrame(() => field.setSelectionRange(start + 2, start + 2))
}
</script>

<!-- A plain-text editor for configuration files, with line numbers. -->
<template>
  <div
    class="flex overflow-hidden rounded-lg border bg-muted/30 font-mono text-[13px] leading-5 focus-within:ring-[3px] focus-within:ring-ring/50"
  >
    <div
      ref="gutter"
      aria-hidden="true"
      class="shrink-0 overflow-hidden border-r bg-muted/50 px-2 py-2 text-right text-muted-foreground select-none"
    >
      <div v-for="line in lines" :key="line">{{ line }}</div>
    </div>
    <textarea
      ref="area"
      v-model="text"
      :aria-label="label"
      :readonly="readonly"
      :rows="Math.max(minRows ?? 16, Math.min(lines + 1, 40))"
      spellcheck="false"
      autocapitalize="off"
      autocomplete="off"
      wrap="off"
      class="min-w-0 flex-1 resize-y bg-transparent px-3 py-2 outline-none"
      @scroll="follow"
      @keydown.tab="indent"
    />
  </div>
</template>
