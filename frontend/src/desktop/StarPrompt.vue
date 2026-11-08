<script setup lang="ts">
import { Star, X } from 'lucide-vue-next'
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { useSessionStore } from '@/features/auth/store'
import { useStorageOverview } from '@/features/storage/api'
import type { StarPromptAnswer } from '@/shared/api/types'
import { useJobsStore } from '@/shared/jobs/store'
import { useNotifications } from '@/shared/lib/notifications'
import {
  DELAY_MS,
  PROJECT_URL,
  RETRY_MS,
  type StarPromptFacts,
  starPromptVerdict,
} from '@/shared/lib/star-prompt'
import { usePreferences } from '@/shared/preferences/store'
import { Button } from '@/shared/ui/button'

import { usePower } from './power'

/** Shown in this browser session: kept until the tab closes. */
const SESSION_KEY = 'zendiloos.starPrompt.shown'
/** Open dialogs, alert dialogs, the launcher, menus. App windows do not count. */
const DIALOGS =
  '[role="dialog"][data-state="open"], [role="alertdialog"], [aria-modal="true"], [role="menu"]'

const { t } = useI18n()
const session = useSessionStore()
const preferences = usePreferences()
const jobs = useJobsStore()
const notifications = useNotifications()
const power = usePower()

/** The storage overview is only asked for while waiting for a calm moment. */
const asking = ref(false)
const { data: overview } = useStorageOverview(asking)

const visible = ref(false)
const card = ref<HTMLElement | null>(null)
let timer: ReturnType<typeof setTimeout> | undefined

function shownThisSession(): boolean {
  try {
    return sessionStorage.getItem(SESSION_KEY) !== null
  } catch {
    return false // The account's preferences still say when it was shown.
  }
}

function facts(): StarPromptFacts {
  const alerts = notifications.items.filter(
    (item) => !item.read && (item.kind === 'warning' || item.kind === 'error'),
  )
  return {
    now: Math.floor(Date.now() / 1000),
    role: session.info?.account.role,
    createdAt: session.info?.account.created_at,
    loaded: preferences.loaded,
    prompt: preferences.starPrompt,
    shownThisSession: shownThisSession(),
    runningJobs: jobs.running.length,
    dialogOpen: power.phase !== 'running' || document.querySelector(DIALOGS) !== null,
    alerts: alerts.length + notifications.toasts.length,
    storage: overview.value === undefined ? undefined : (overview.value.verdict ?? null),
  }
}

function consider(): void {
  const verdict = starPromptVerdict(facts())
  asking.value = verdict === 'wait'
  if (verdict === 'show') show()
  else if (verdict === 'wait') timer = setTimeout(consider, RETRY_MS)
}

function show(): void {
  visible.value = true
  try {
    sessionStorage.setItem(SESSION_KEY, '1')
  } catch {
    // The account's preferences still say when it was shown.
  }
  preferences.starPromptShown(Math.floor(Date.now() / 1000))
  document.addEventListener('keydown', escape)
}

function answer(choice: StarPromptAnswer): void {
  visible.value = false
  document.removeEventListener('keydown', escape)
  preferences.answerStarPrompt(choice)
}

/** Escape closes it, as "Later": unless it is meant for a window or a dialog. */
function escape(event: KeyboardEvent): void {
  if (event.key !== 'Escape' || event.defaultPrevented) return
  const focus = document.activeElement
  if (focus && focus !== document.body && !card.value?.contains(focus)) return
  answer('later')
}

onMounted(() => {
  timer = setTimeout(consider, DELAY_MS)
})

onBeforeUnmount(() => {
  clearTimeout(timer)
  document.removeEventListener('keydown', escape)
})
</script>

<!--
  Asks an administrator, once and perhaps once more, for a star on GitHub:
  a card where the toasts appear, never a dialog. The button is only a link;
  nothing is counted or sent.
-->
<template>
  <Transition
    enter-from-class="translate-x-4 opacity-0"
    leave-to-class="translate-x-4 opacity-0"
    enter-active-class="transition duration-200"
    leave-active-class="transition duration-200"
  >
    <section
      v-if="visible"
      ref="card"
      :aria-label="t('desktop.starPrompt.label')"
      class="pointer-events-auto flex gap-3 rounded-lg border bg-card p-3 shadow-lg"
    >
      <Star class="mt-0.5 size-4 shrink-0 fill-amber-400 text-amber-500" aria-hidden="true" />
      <div class="min-w-0 flex-1">
        <p class="text-sm">{{ t('desktop.starPrompt.text') }}</p>
        <div class="mt-2.5 flex flex-wrap gap-1.5">
          <Button
            as="a"
            size="sm"
            :href="PROJECT_URL"
            target="_blank"
            rel="noopener noreferrer"
            @click="answer('starred')"
          >
            {{ t('desktop.starPrompt.view') }}
          </Button>
          <Button size="sm" variant="ghost" @click="answer('later')">
            {{ t('desktop.starPrompt.later') }}
          </Button>
          <Button size="sm" variant="ghost" @click="answer('declined')">
            {{ t('desktop.starPrompt.decline') }}
          </Button>
        </div>
      </div>
      <button
        type="button"
        class="self-start rounded p-0.5 hover:bg-accent"
        :aria-label="t('desktop.close')"
        @click="answer('later')"
      >
        <X class="size-4" />
      </button>
    </section>
  </Transition>
</template>
