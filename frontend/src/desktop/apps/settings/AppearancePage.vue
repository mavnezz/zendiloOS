<script setup lang="ts">
import { Check, Monitor, Moon, Sun } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { LANGUAGES, type Locale } from '@/shared/i18n'
import type { ThemePreference } from '@/shared/lib/theme'
import { cn } from '@/shared/lib/utils'
import {
  isHexColor,
  SOLID_COLORS,
  SOLID_WALLPAPER,
  solidWallpaper,
  toneOf,
  type Wallpaper,
  WALLPAPERS,
} from '@/shared/lib/wallpapers'
import { usePreferences } from '@/shared/preferences/store'
import { PageCard, PageHeader } from '@/shared/ui/page'

const { t, locale } = useI18n()
const preferences = usePreferences()

/** The tile of the solid background, in the colour last chosen for it. */
const solid = computed(() => solidWallpaper(preferences.solidColor))

/** A tile's name stays readable on its picture, light or dark, in either scheme. */
function label(wallpaper: Wallpaper): string {
  return wallpaper.tone === 'dark'
    ? 'text-white [text-shadow:0_1px_2px_rgb(0_0_0/0.6)]'
    : 'text-neutral-900 [text-shadow:0_1px_2px_rgb(255_255_255/0.5)]'
}

function picked(event: Event): string | null {
  const value = (event.target as HTMLInputElement).value.toLowerCase()
  return isHexColor(value) ? value : null
}

/** While the picker is open: shown at once, kept when it closes. */
function preview(event: Event): void {
  const color = picked(event)
  if (color) preferences.showWallpaper(solidWallpaper(color))
}

function pick(event: Event): void {
  const color = picked(event)
  if (color) preferences.chooseWallpaper(solidWallpaper(color))
}

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
        <div class="grid gap-4">
          <div
            class="grid grid-cols-2 gap-3 @xl:grid-cols-3"
            role="radiogroup"
            :aria-label="t('appearance.wallpaper')"
          >
            <button
              v-for="wallpaper in [...WALLPAPERS, solid]"
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
              <span :class="cn('absolute bottom-2 left-2 text-xs font-medium', label(wallpaper))">
                {{ wallpaper.id === SOLID_WALLPAPER ? t('appearance.solid') : wallpaper.name }}
              </span>
              <span
                v-if="preferences.wallpaper.id === wallpaper.id"
                class="absolute top-2 right-2 flex size-5 items-center justify-center rounded-full bg-primary text-primary-foreground"
              >
                <Check class="size-3" />
              </span>
            </button>
          </div>

          <!-- The colour of the solid background: a few that suit the desktop, or any. -->
          <div
            v-if="preferences.wallpaper.id === SOLID_WALLPAPER"
            class="flex flex-wrap items-center gap-2.5"
          >
            <div
              class="flex flex-wrap gap-2.5"
              role="radiogroup"
              :aria-label="t('appearance.colour')"
            >
              <button
                v-for="swatch in SOLID_COLORS"
                :key="swatch.id"
                type="button"
                role="radio"
                :aria-checked="preferences.solidColor === swatch.color"
                :aria-label="t(`appearance.colours.${swatch.id}`)"
                :title="t(`appearance.colours.${swatch.id}`)"
                :class="
                  cn(
                    'flex size-8 items-center justify-center rounded-full ring-1 ring-foreground/15 outline-none transition focus-visible:ring-2 focus-visible:ring-ring',
                    preferences.solidColor === swatch.color &&
                      'ring-2 ring-primary ring-offset-2 ring-offset-card',
                  )
                "
                :style="{ backgroundColor: swatch.color }"
                @click="preferences.chooseWallpaper(solidWallpaper(swatch.color))"
              >
                <Check
                  v-if="preferences.solidColor === swatch.color"
                  :class="
                    cn(
                      'size-4',
                      toneOf(swatch.color) === 'dark' ? 'text-white' : 'text-neutral-900',
                    )
                  "
                />
              </button>
            </div>
            <label class="flex items-center gap-2 text-xs text-muted-foreground">
              <input
                type="color"
                :value="preferences.solidColor"
                :aria-label="t('appearance.pickColour')"
                class="size-8 cursor-pointer rounded-full border-0 bg-transparent p-0 ring-1 ring-foreground/15 outline-none focus-visible:ring-2 focus-visible:ring-ring [&::-moz-color-swatch]:rounded-full [&::-moz-color-swatch]:border-0 [&::-webkit-color-swatch]:rounded-full [&::-webkit-color-swatch]:border-0 [&::-webkit-color-swatch-wrapper]:p-0"
                @input="preview"
                @change="pick"
              />
              <span class="font-mono tabular-nums">{{ preferences.solidColor }}</span>
            </label>
          </div>
        </div>
      </PageCard>
    </div>
  </div>
</template>
