<script setup lang="ts">
import { Boxes } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'

import type { DockerProject } from '@/shared/api/types'

/** A project's tile: the app's icon for one made by the App Center, else boxes. */
const props = defineProps<{ project?: Pick<DockerProject, 'template'> | null }>()

const failed = ref(false)
const src = computed(() => {
  const template = props.project?.template
  if (!template || failed.value) return null
  return `/api/v1/apps/catalog/${template.source_id}/${encodeURIComponent(template.app)}/icon`
})
watch(
  () => props.project?.template,
  () => (failed.value = false),
)
</script>

<template>
  <span class="grid size-12 shrink-0 place-items-center rounded-xl bg-primary/10 text-primary">
    <img
      v-if="src"
      :src="src"
      alt=""
      class="size-9 rounded-lg object-contain"
      @error="failed = true"
    />
    <Boxes v-else class="size-6" />
  </span>
</template>
