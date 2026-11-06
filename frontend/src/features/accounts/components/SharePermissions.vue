<script setup lang="ts">
import { FolderLock } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Share, ShareRuleLevel } from '@/shared/api/types'
import { cn } from '@/shared/lib/utils'
import { Badge } from '@/shared/ui/badge'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import type { Effective } from '../lib/access'

/** The columns, strongest first: a denial wins over everything. */
const LEVELS: ShareRuleLevel[] = ['deny', 'read_write', 'read']

const props = defineProps<{
  shares: Share[]
  /** For an account: what it may do in the end, with its groups. */
  effective?: (share: Share, rule: ShareRuleLevel | null) => Effective
  disabled?: boolean
}>()
/** The rule per share ID; `null` or missing: none. */
const rules = defineModel<Record<number, ShareRuleLevel | null>>({ required: true })

const { t } = useI18n()

function set(share: number, level: ShareRuleLevel, on: boolean): void {
  rules.value = { ...rules.value, [share]: on ? level : null }
}

/** Header boxes: all shares at once. */
const all = computed(
  () =>
    Object.fromEntries(
      LEVELS.map((level) => [
        level,
        props.shares.length > 0 && props.shares.every((share) => rules.value[share.id] === level),
      ]),
    ) as Record<ShareRuleLevel, boolean>,
)
function setAll(level: ShareRuleLevel, on: boolean): void {
  rules.value = Object.fromEntries(props.shares.map((share) => [share.id, on ? level : null]))
}

const tone: Record<NonNullable<Effective> | 'none', string> = {
  read_write: 'success',
  read: 'info',
  none: 'destructive',
}
</script>

<!-- One row per shared folder: deny, read and write, or read only; none of them: no rule. -->
<template>
  <p v-if="!shares.length" class="py-8 text-center text-sm text-muted-foreground">
    {{ t('people.noShares') }}
  </p>
  <Table v-else>
    <TableHeader>
      <TableRow class="hover:bg-transparent">
        <TableHead class="w-full">{{ t('people.sharedFolder') }}</TableHead>
        <TableHead v-if="effective">{{ t('people.result') }}</TableHead>
        <TableHead v-for="level in LEVELS" :key="level">
          <label class="flex items-center gap-2">
            <input
              type="checkbox"
              class="size-3.5 accent-primary"
              :checked="all[level]"
              :disabled="disabled"
              :aria-label="t('people.allShares', { level: t(`people.levels.${level}`) })"
              @change="setAll(level, ($event.target as HTMLInputElement).checked)"
            />
            {{ t(`people.levels.${level}`) }}
          </label>
        </TableHead>
      </TableRow>
    </TableHeader>
    <TableBody>
      <TableRow v-for="share in shares" :key="share.id">
        <TableCell>
          <span class="flex items-center gap-2 font-medium">
            <FolderLock class="size-4 text-amber-500" aria-hidden="true" />
            {{ share.name }}
          </span>
        </TableCell>
        <TableCell v-if="effective">
          <Badge
            :variant="tone[effective(share, rules[share.id] ?? null) ?? 'none'] as 'success'"
            class="whitespace-nowrap"
          >
            {{ t(`people.effective.${effective(share, rules[share.id] ?? null) ?? 'none'}`) }}
          </Badge>
        </TableCell>
        <TableCell v-for="level in LEVELS" :key="level">
          <input
            type="checkbox"
            :class="cn('size-4', level === 'deny' ? 'accent-destructive' : 'accent-primary')"
            :checked="rules[share.id] === level"
            :disabled="disabled"
            :aria-label="`${share.name}: ${t(`people.levels.${level}`)}`"
            @change="set(share.id, level, ($event.target as HTMLInputElement).checked)"
          />
        </TableCell>
      </TableRow>
    </TableBody>
  </Table>
</template>
