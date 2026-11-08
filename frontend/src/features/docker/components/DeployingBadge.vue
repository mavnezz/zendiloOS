<script setup lang="ts">
import { LoaderCircle } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

/** A deployment of the project runs now: says so, and opens its output. */
const emit = defineEmits<{ show: [] }>()
const { t } = useI18n()
</script>

<template>
  <button
    type="button"
    class="inline-flex items-center gap-1.5 rounded-md bg-primary/15 px-2 py-0.5 text-xs font-medium text-primary outline-none hover:bg-primary/25 focus-visible:ring-2 focus-visible:ring-ring"
    :title="t('docker.deploy.show')"
    @click.stop="emit('show')"
  >
    <LoaderCircle class="size-3 animate-spin" />
    {{ t('docker.deploy.running') }}
  </button>
</template>
