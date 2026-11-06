<script setup lang="ts">
import { Check, Monitor, Moon, Sun } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import { LANGUAGES, type Locale } from '@/shared/i18n'
import type { ThemePreference } from '@/shared/lib/theme'
import { cn } from '@/shared/lib/utils'
import { WALLPAPERS } from '@/shared/lib/wallpapers'
import { usePreferences } from '@/shared/preferences/store'
import { PageCard, PageHeader } from '@/shared/ui/page'

const { t, locale } = useI18n()
const preferences = usePreferences()

const themes: { id: ThemePreference; icon: typeof Sun }[] = [
  { id: 'light', icon: Sun },
  { id: 'dark', icon: Moon },
  { id: 'system', icon: Monitor },
]
</script>

<!-- How zendiloOS looks for this account, in every browser. -->
<template>
  <div>
    <PageHeader :title="t('controlPanel.appearance')" :description="t('appearance.description')" />
    <div class="grid max-w-3xl gap-4">
      <PageCard :title="t('appearance.language')" :description="t('appearance.languageHint')">
        <select
          :value="locale"
          class="h-9 w-full max-w-80 rounded-md border bg-background px-3 text-sm shadow-xs outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
          :aria-label="t('appearance.language')"
          @change="preferences.chooseLanguage(($event.target as HTMLSelectElement).value as Locale)"
        >
          <option v-for="language in LANGUAGES" :key="language.locale" :value="language.locale">
            {{ language.name }}
          </option>
        </select>
      </PageCard>

      <PageCard :title="t('appearance.theme')">
        <div class="grid grid-cols-3 gap-2" role="radiogroup" :aria-label="t('appearance.theme')">
          <button
            v-for="option in themes"
            :key="option.id"
            type="button"
            role="radio"
            :aria-checked="preferences.theme === option.id"
            :class="
              cn(
                'flex flex-col items-center gap-2 rounded-lg border px-3 py-4 text-sm font-medium outline-none transition hover:bg-muted/60 focus-visible:ring-2 focus-visible:ring-ring',
                preferences.theme === option.id &&
                  'border-primary bg-primary/5 text-primary hover:bg-primary/10',
              )
            "
            @click="preferences.chooseTheme(option.id)"
          >
            <component :is="option.icon" class="size-5" />
            {{ t(`appearance.themes.${option.id}`) }}
          </button>
        </div>
      </PageCard>

      <PageCard :title="t('appearance.wallpaper')">
        <div
          class="grid grid-cols-2 gap-3 @xl:grid-cols-3"
          role="radiogroup"
          :aria-label="t('appearance.wallpaper')"
        >
          <button
            v-for="wallpaper in WALLPAPERS"
            :key="wallpaper.id"
            type="button"
            role="radio"
            :aria-checked="preferences.wallpaper.id === wallpaper.id"
            :class="
              cn(
                'relative aspect-[16/9] overflow-hidden rounded-lg bg-cover bg-center text-left ring-1 ring-foreground/10 outline-none transition focus-visible:ring-2 focus-visible:ring-ring',
                preferences.wallpaper.id === wallpaper.id && 'ring-2 ring-primary',
              )
            "
            :style="{ backgroundColor: wallpaper.color, backgroundImage: wallpaper.image }"
            @click="preferences.chooseWallpaper(wallpaper)"
          >
            <span
              :class="
                cn(
                  'absolute bottom-2 left-2 text-xs font-medium',
                  wallpaper.tone === 'dark'
                    ? 'text-white [text-shadow:0_1px_2px_rgb(0_0_0/0.6)]'
                    : 'text-foreground',
                )
              "
            >
              {{ wallpaper.name }}
            </span>
            <span
              v-if="preferences.wallpaper.id === wallpaper.id"
              class="absolute top-2 right-2 flex size-5 items-center justify-center rounded-full bg-primary text-primary-foreground"
            >
              <Check class="size-3" />
            </span>
          </button>
        </div>
      </PageCard>
    </div>
  </div>
</template>
