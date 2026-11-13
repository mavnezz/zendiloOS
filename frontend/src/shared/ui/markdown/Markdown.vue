<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'

import { renderMarkdown } from '@/shared/lib/markdown'
import { cn } from '@/shared/lib/utils'

const props = defineProps<{ source: string; class?: HTMLAttributes['class'] }>()
const html = computed(() => renderMarkdown(props.source))
const classes = computed(() =>
  cn(
    'grid gap-2 text-dense leading-relaxed break-words',
    '[&_h1]:font-semibold [&_h2]:font-semibold [&_h3]:font-semibold [&_h4]:font-medium [&_h5]:font-medium [&_h6]:font-medium',
    '[&_ol]:list-decimal [&_ol]:pl-5 [&_ul]:list-disc [&_ul]:pl-5 [&_li>ol]:mt-0.5 [&_li>ul]:mt-0.5',
    '[&_a]:text-primary [&_a]:underline [&_a]:underline-offset-2',
    '[&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_code]:font-mono [&_code]:text-xs',
    '[&_pre]:overflow-auto [&_pre]:rounded-md [&_pre]:bg-muted [&_pre]:p-2 [&_pre_code]:bg-transparent [&_pre_code]:p-0',
    '[&_blockquote]:border-l-2 [&_blockquote]:pl-3 [&_blockquote]:text-muted-foreground',
    '[&_td]:pr-4 [&_th]:pr-4 [&_th]:text-left [&_th]:font-medium',
    props.class,
  ),
)
</script>

<!-- Release notes and other Markdown: headings, lists, emphasis, code and links; HTML in it shows as text. -->
<template>
  <!-- eslint-disable-next-line vue/no-v-html -- renderMarkdown escapes the source's HTML -->
  <div :class="classes" v-html="html" />
</template>
