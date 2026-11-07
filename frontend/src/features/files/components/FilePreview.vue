<script setup lang="ts">
import { Download } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'

import { downloadUrl, previewKind, previewUrl, textPreview } from '../api'
import type { Place } from '../lib/location'

/** The file to show: its place and path, in a snapshot if given; null shows nothing. */
const props = defineProps<{ share: Place; path: string | null; snapshot?: string | null }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()

const name = computed(() => props.path?.split('/').pop() ?? '')
const kind = computed(() => (props.path ? previewKind(name.value) : null))
const source = computed(() =>
  props.path ? previewUrl(props.share, props.path, props.snapshot) : '',
)

const text = ref<{ text: string; more: boolean } | null>(null)
const failure = ref<string | null>(null)

watch(
  [open, source],
  async ([isOpen, url]) => {
    text.value = null
    failure.value = null
    if (!isOpen || kind.value !== 'text' || !url) return
    try {
      const loaded = await textPreview(url)
      // Only if the dialog still shows this file.
      if (source.value === url) text.value = loaded
    } catch (problem) {
      failure.value = problemMessage(problem)
    }
  },
  { immediate: true },
)
</script>

<!-- A picture, sound, video or text file, shown without saving it. -->
<template>
  <ModalDialog
    v-model:open="open"
    wide
    :title="t('files.previewTitle', { name })"
    :close-label="t('desktop.close')"
  >
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <img
      v-if="kind === 'image'"
      :src="source"
      :alt="name"
      class="mx-auto max-h-[70dvh] max-w-full object-contain"
    />
    <video
      v-else-if="kind === 'video'"
      :src="source"
      controls
      preload="metadata"
      class="mx-auto max-h-[70dvh] max-w-full"
    />
    <audio v-else-if="kind === 'audio'" :src="source" controls preload="metadata" class="w-full" />
    <template v-else-if="kind === 'text' && text">
      <pre
        class="max-h-[60dvh] overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs whitespace-pre-wrap"
        >{{ text.text }}</pre>
      <p v-if="text.more" class="text-xs text-muted-foreground">{{ t('files.textTruncated') }}</p>
    </template>
    <div v-if="path" class="flex justify-end">
      <Button
        as="a"
        variant="outline"
        size="sm"
        :href="downloadUrl(share, path, snapshot)"
        download
      >
        <Download />
        {{ t('files.download') }}
      </Button>
    </div>
  </ModalDialog>
</template>
