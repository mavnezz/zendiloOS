<script setup lang="ts">
import { Info } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

defineProps<{ title?: string }>()
const { t } = useI18n()
</script>

<!--
  What a page does, in plain words, at its end: a NAS is run by people who
  are not administrators by trade, and a page of switches leaves them
  guessing.
-->
<template>
  <section class="mt-8 border-t pt-5 text-sm text-muted-foreground">
    <h3 class="mb-2 flex items-center gap-1.5 font-medium text-foreground">
      <Info class="size-4" />
      {{ title ?? t('common.description') }}
    </h3>
    <div class="space-y-2">
      <slot />
    </div>
  </section>
</template>
