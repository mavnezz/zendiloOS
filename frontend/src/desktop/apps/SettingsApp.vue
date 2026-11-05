<script setup lang="ts">
import { Info, Palette, UserRound } from 'lucide-vue-next'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import ChangePasswordForm from '@/features/auth/components/ChangePasswordForm.vue'
import { useSessionStore } from '@/features/auth/store'
import AboutSection from '@/features/system/components/AboutSection.vue'
import { LANGUAGES } from '@/shared/i18n'
import type { ThemePreference } from '@/shared/lib/theme'
import { cn } from '@/shared/lib/utils'
import { usePreferences } from '@/shared/preferences/store'

type Section = 'account' | 'appearance' | 'about'

const { t, locale } = useI18n()
const session = useSessionStore()
const preferences = usePreferences()

const sections = [
  { id: 'account', icon: UserRound },
  { id: 'appearance', icon: Palette },
  { id: 'about', icon: Info },
] as const
const section = ref<Section>('account')

const themes: ThemePreference[] = ['light', 'dark', 'system']

const choice = (selected: boolean) =>
  cn(
    'rounded-md px-3 py-1.5 text-sm transition-colors',
    selected ? 'bg-card font-medium shadow-sm' : 'text-muted-foreground hover:text-foreground',
  )
</script>

<!-- Settings of the signed-in account and of this browser. -->
<template>
  <div class="flex h-full flex-col @xl:flex-row">
    <nav class="flex shrink-0 gap-1 border-b p-2 @xl:w-48 @xl:flex-col @xl:border-r @xl:border-b-0">
      <button
        v-for="item in sections"
        :key="item.id"
        type="button"
        :class="
          cn(
            'flex items-center gap-2 rounded-md px-3 py-2 text-sm',
            section === item.id ? 'bg-accent font-medium' : 'hover:bg-accent/60',
          )
        "
        :aria-current="section === item.id ? 'page' : undefined"
        @click="section = item.id"
      >
        <component :is="item.icon" class="size-4" />
        {{ t(`settings.${item.id}`) }}
      </button>
    </nav>

    <div class="min-w-0 flex-1 overflow-auto p-6">
      <section v-if="section === 'account' && session.info" class="grid gap-6">
        <div>
          <h3 class="text-lg font-semibold">
            {{ t('settings.signedInAs', { name: session.info.account.name }) }}
          </h3>
          <p class="text-sm text-muted-foreground">
            {{ t('settings.role') }}: {{ t(`settings.roles.${session.info.account.role}`) }}
          </p>
        </div>
        <div class="grid gap-3">
          <h4 class="font-medium">{{ t('settings.changePassword') }}</h4>
          <ChangePasswordForm />
        </div>
      </section>

      <section v-else-if="section === 'appearance'" class="grid gap-6">
        <div class="grid gap-2">
          <h3 class="font-medium">{{ t('settings.theme') }}</h3>
          <div class="inline-flex w-fit gap-1 rounded-lg bg-muted p-1" role="radiogroup">
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
        </div>
        <div class="grid gap-2">
          <h3 class="font-medium">{{ t('settings.language') }}</h3>
          <div class="inline-flex w-fit gap-1 rounded-lg bg-muted p-1" role="radiogroup">
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
        </div>
      </section>

      <section v-else-if="section === 'about'">
        <AboutSection />
      </section>
    </div>
  </div>
</template>
