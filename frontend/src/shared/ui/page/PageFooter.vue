<script setup lang="ts">
import { CircleAlert, LoaderCircle } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import { Button } from '@/shared/ui/button'

withDefaults(
  defineProps<{
    dirty: boolean
    saving?: boolean
    error?: string | null
    /** False while the page shows an input it would not send. */
    valid?: boolean
  }>(),
  { valid: true, error: null },
)
const emit = defineEmits<{ apply: []; reset: [] }>()
const { t } = useI18n()
</script>

<!--
  Apply and reset, along the bottom of a settings page, once there is
  something to apply. Sticky rather than fixed: it belongs to the page, and
  a window can be made shorter than the page. It reaches over the frame's
  padding (--frame-padding, set by AppFrame) to the window's edges.
-->
<template>
  <footer
    v-if="dirty || error"
    class="sticky -bottom-(--frame-padding,0px) -mx-(--frame-padding,0px) mt-6 -mb-(--frame-padding,0px) flex flex-wrap items-center justify-end gap-3 border-t bg-background/92 px-(--frame-padding,1rem) py-3 backdrop-blur"
  >
    <p v-if="error" role="alert" class="mr-auto flex items-center gap-1.5 text-sm text-destructive">
      <CircleAlert class="size-4 shrink-0" />
      {{ error }}
    </p>
    <Button variant="ghost" :disabled="saving" @click="emit('reset')">{{
      t('common.reset')
    }}</Button>
    <Button :disabled="!dirty || saving || !valid" @click="emit('apply')">
      <LoaderCircle v-if="saving" class="animate-spin" />
      {{ t('common.apply') }}
    </Button>
  </footer>
</template>
