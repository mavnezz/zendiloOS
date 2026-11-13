<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

const props = defineProps<{ text?: string; loading?: boolean }>()
const { t } = useI18n()
const box = ref<HTMLElement | null>(null)

// New lines come at the end: stay there, unless the reader scrolled up.
watch(
  () => props.text,
  async () => {
    const element = box.value
    const atEnd = !element || element.scrollHeight - element.scrollTop - element.clientHeight < 40
    await nextTick()
    if (atEnd && box.value) box.value.scrollTop = box.value.scrollHeight
  },
  { immediate: true },
)
</script>

<!-- What containers wrote, as a terminal shows it. -->
<template>
  <pre
    ref="box"
    class="max-h-[60vh] min-h-48 overflow-auto rounded-lg bg-zinc-900 p-3 font-mono text-xs leading-5 whitespace-pre-wrap text-zinc-100"
    >{{ loading ? t('docker.logs.loading') : text?.trim() || t('docker.logs.none') }}</pre>
</template>
