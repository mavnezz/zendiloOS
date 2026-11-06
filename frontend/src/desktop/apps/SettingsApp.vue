<script setup lang="ts">
import { BellRing, Info, Network, Palette, UserRound } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import ChangePasswordForm from '@/features/auth/components/ChangePasswordForm.vue'
import { useSessionStore } from '@/features/auth/store'
import ChannelsSection from '@/features/channels/components/ChannelsSection.vue'
import NetworkSection from '@/features/network/components/NetworkSection.vue'
import AboutSection from '@/features/system/components/AboutSection.vue'
import { LANGUAGES } from '@/shared/i18n'
import type { ThemePreference } from '@/shared/lib/theme'
import { cn } from '@/shared/lib/utils'
import { AppFrame, type NavGroup } from '@/shared/ui/frame'
import { PanelSection } from '@/shared/ui/section'
import { usePreferences } from '@/shared/preferences/store'

const { t, locale } = useI18n()
const session = useSessionStore()
const preferences = usePreferences()

const groups = computed<NavGroup[]>(() => [
  {
    id: 'personal',
    label: t('settings.groups.personal'),
    items: [
      { id: 'account', label: t('settings.account'), icon: UserRound, tone: 'blue' },
      { id: 'appearance', label: t('settings.appearance'), icon: Palette, tone: 'violet' },
    ],
  },
  {
    id: 'system',
    label: t('settings.groups.system'),
    items: [
      { id: 'network', label: t('settings.network'), icon: Network, tone: 'sky' },
      // Notification channels are for administrators (`system.manage`).
      ...(session.can('system.manage')
        ? [
            {
              id: 'notifications',
              label: t('settings.notifications'),
              icon: BellRing,
              tone: 'amber' as const,
            },
          ]
        : []),
      { id: 'about', label: t('settings.about'), icon: Info, tone: 'slate' },
    ],
  },
])
const section = ref('account')

const themes: ThemePreference[] = ['light', 'dark', 'system']

const choice = (selected: boolean) =>
  cn(
    'rounded-[5px] px-3 py-1 text-sm transition-colors',
    selected ? 'bg-card font-medium shadow-xs' : 'text-muted-foreground hover:text-foreground',
  )
</script>

<!-- Settings of the signed-in account and of this browser. -->
<template>
  <AppFrame v-model:section="section" :groups="groups">
    <div v-if="section === 'account' && session.info" class="grid max-w-xl gap-6">
      <div class="flex items-center gap-3">
        <span
          class="grid size-11 place-items-center rounded-full bg-primary/10 text-base font-semibold text-primary uppercase"
        >
          {{ session.info.account.name.slice(0, 2) }}
        </span>
        <div class="grid">
          <h3 class="text-base font-semibold">
            {{ t('settings.signedInAs', { name: session.info.account.name }) }}
          </h3>
          <p class="text-xs text-muted-foreground">
            {{ t('settings.role') }}: {{ t(`settings.roles.${session.info.account.role}`) }}
          </p>
        </div>
      </div>
      <PanelSection :title="t('settings.changePassword')">
        <ChangePasswordForm />
      </PanelSection>
    </div>

    <div v-else-if="section === 'appearance'" class="grid gap-6">
      <PanelSection :title="t('settings.theme')">
        <div class="inline-flex w-fit gap-0.5 rounded-md bg-muted p-0.5" role="radiogroup">
          <button
            v-for="option in themes"
            :key="option"
            type="button"
            role="radio"
            :aria-checked="preferences.theme === option"
            :class="choice(preferences.theme === option)"
            @click="preferences.chooseTheme(option)"
          >
            {{ t(`settings.themes.${option}`) }}
          </button>
        </div>
      </PanelSection>
      <PanelSection :title="t('settings.language')">
        <div class="inline-flex w-fit gap-0.5 rounded-md bg-muted p-0.5" role="radiogroup">
          <button
            v-for="language in LANGUAGES"
            :key="language.locale"
            type="button"
            role="radio"
            :lang="language.locale"
            :aria-checked="locale === language.locale"
            :class="choice(locale === language.locale)"
            @click="preferences.chooseLanguage(language.locale)"
          >
            {{ language.name }}
          </button>
        </div>
      </PanelSection>
    </div>

    <NetworkSection v-else-if="section === 'network'" :can-manage="session.can('system.manage')" />
    <ChannelsSection v-else-if="section === 'notifications'" />
    <AboutSection v-else-if="section === 'about'" />
  </AppFrame>
</template>
