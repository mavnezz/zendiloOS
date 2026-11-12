<script setup lang="ts">
import { X } from 'lucide-vue-next'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'

import { addPattern, isPattern } from '../lib/backup'

/** rsync's exclude patterns as chips: added by typing, removed by their ×. */
const patterns = defineModel<string[]>({ required: true })

const { t } = useI18n()
const typed = ref('')
const bad = ref(false)

function add(): void {
  const pattern = typed.value.trim()
  if (!pattern) return
  if (!isPattern(pattern)) {
    bad.value = true
    return
  }
  patterns.value = addPattern(patterns.value, pattern)
  typed.value = ''
  bad.value = false
}
</script>

<template>
  <div class="grid gap-2">
    <ul class="flex flex-wrap gap-1.5" :aria-label="t('backup.wizard.filters')">
      <li
        v-for="pattern in patterns"
        :key="pattern"
        class="flex items-center gap-1 rounded-full bg-foreground/[0.06] py-0.5 pr-1 pl-2.5 font-mono text-xs"
      >
        {{ pattern }}
        <button
          type="button"
          class="rounded-full p-0.5 text-muted-foreground hover:bg-foreground/10 hover:text-foreground"
          :aria-label="t('backup.wizard.removePattern', { pattern })"
          @click="patterns = patterns.filter((other) => other !== pattern)"
        >
          <X class="size-3" />
        </button>
      </li>
    </ul>
    <div class="flex gap-2">
      <Input
        id="backup-pattern"
        v-model="typed"
        class="max-w-xs font-mono"
        :placeholder="t('backup.wizard.addPattern')"
        :aria-label="t('backup.wizard.addPattern')"
        @keydown.enter.prevent="add"
      />
      <Button type="button" variant="outline" size="sm" @click="add">{{
        t('backup.wizard.add')
      }}</Button>
    </div>
    <p v-if="bad" class="text-xs text-destructive">{{ t('backup.wizard.badPattern') }}</p>
  </div>
</template>
