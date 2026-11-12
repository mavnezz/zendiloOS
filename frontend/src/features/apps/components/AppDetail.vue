<script setup lang="ts">
import { ArrowLeft, Blocks, ShieldAlert } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'

import { iconUrl, useCatalogApp, useDocker, useSources } from '../api'
import { usePresence } from '../lib/presence'
import { appText, asksForMore, paragraphs } from '../texts'
import InstallDialog from './InstallDialog.vue'
import PermissionList from './PermissionList.vue'

/** `install`: begin the installation as soon as the app is known. */
const props = defineProps<{ source: number; id: string; install?: boolean }>()
const emit = defineEmits<{ back: []; installed: [] }>()

const { t, locale } = useI18n()
const which = computed(() => ({ source: props.source, id: props.id }))
const { data: app, error } = useCatalogApp(which)
const { data: docker } = useDocker()
const { data: sources } = useSources()
const presenceOf = usePresence()
const installing = ref(false)
const brokenIcon = ref(false)
const showCompose = ref(false)

/** Whether a project was made from the app here, or it was installed the earlier way. */
const presence = computed(() => presenceOf({ source_id: props.source, id: props.id }))
const dockerReady = computed(() => Boolean(docker.value?.volume && docker.value.running))
/** Its images are made for this NAS's architecture; otherwise it cannot be installed. */
const runsHere = computed(() => app.value?.runs_here !== false)
const catalog = computed(() => {
  if (app.value?.official) return t('appStore.sources.official')
  const source = sources.value?.find((s) => s.id === props.source)
  return source?.address.replace(/^https:\/\//, '') ?? '–'
})
const kinds = computed(() =>
  (app.value?.categories ?? []).map((category) => t(`appStore.categories.${category}`)).join(' · '),
)
const notes = computed(() =>
  app.value ? paragraphs(appText(app.value, 'notes', locale.value)) : [],
)
const asked = computed(() => (app.value?.settings ?? []).filter((setting) => !setting.generate))
const generated = computed(() => (app.value?.settings ?? []).filter((setting) => setting.generate))
const asksMore = computed(() => Boolean(app.value) && asksForMore(app.value!.permissions))

watch(
  [app, dockerReady],
  ([known, ready]) => {
    if (props.install && known && ready && known.runs_here) installing.value = true
  },
  { immediate: true },
)

const fact = 'grid gap-1 px-3 text-center not-first:border-l'
const heading = 'text-base font-semibold'
</script>

<!-- One app: what it is, who made it, and what it needs from the NAS. -->
<template>
  <div class="grid gap-6">
    <Button
      variant="ghost"
      size="icon"
      class="-ml-2"
      :aria-label="t('appStore.back')"
      @click="emit('back')"
    >
      <ArrowLeft />
    </Button>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <template v-if="app">
      <header class="flex flex-wrap items-center gap-5 px-2">
        <span
          class="grid size-20 shrink-0 place-items-center overflow-hidden rounded-2xl border bg-card shadow-sm"
        >
          <Blocks v-if="brokenIcon" class="size-9 text-muted-foreground" />
          <img
            v-else
            :src="iconUrl(app)"
            alt=""
            class="size-full object-cover"
            @error="brokenIcon = true"
          />
        </span>
        <div class="grid min-w-0 flex-1 gap-1">
          <h2 class="flex flex-wrap items-center gap-2 text-xl font-semibold">
            {{ appText(app, 'name', locale) }}
            <Badge v-if="!app.official" variant="warning">{{ t('appStore.unofficial') }}</Badge>
          </h2>
          <p class="text-sm text-muted-foreground">{{ kinds }}</p>
        </div>
        <div class="flex flex-wrap items-center gap-2">
          <Button
            v-if="presence.kind === 'open'"
            as="a"
            size="lg"
            class="rounded-full px-6"
            :href="presence.url"
            target="_blank"
            rel="noopener noreferrer"
            >{{ t('appStore.installed.open') }}</Button
          >
          <Button
            v-else-if="presence.kind !== 'install'"
            size="lg"
            variant="outline"
            class="rounded-full px-6"
            @click="emit('installed')"
            >{{ t('appStore.isInstalled') }}</Button
          >
          <!-- A project of one's own: another can be made from the same app. -->
          <Button
            size="lg"
            :variant="presence.kind === 'install' ? 'default' : 'ghost'"
            class="rounded-full px-6"
            :disabled="!dockerReady || !runsHere"
            :title="
              !runsHere
                ? t('appStore.notForThisNas')
                : dockerReady
                  ? undefined
                  : t('appStore.dockerFirst')
            "
            @click="installing = true"
          >
            {{ presence.kind === 'install' ? t('appStore.install') : t('appStore.installAgain') }}
          </Button>
        </div>
      </header>

      <dl class="grid grid-cols-2 gap-y-4 border-y py-4 text-sm @2xl:grid-cols-5">
        <div :class="fact">
          <dt class="text-xs text-muted-foreground">{{ t('appStore.developer') }}</dt>
          <dd>
            <a
              :href="app.website"
              target="_blank"
              rel="noopener noreferrer"
              class="text-primary hover:underline"
              >{{ app.developer }}</a
            >
          </dd>
        </div>
        <div :class="fact">
          <dt class="text-xs text-muted-foreground">{{ t('appStore.catalog') }}</dt>
          <dd class="truncate">{{ catalog }}</dd>
        </div>
        <div :class="fact">
          <dt class="text-xs text-muted-foreground">{{ t('appStore.version') }}</dt>
          <dd class="font-medium">{{ app.version ?? t('appStore.rolling') }}</dd>
        </div>
        <div :class="fact">
          <dt class="text-xs text-muted-foreground">{{ t('appStore.license') }}</dt>
          <dd>{{ app.license }}</dd>
        </div>
        <div :class="fact">
          <dt class="text-xs text-muted-foreground">{{ t('appStore.guide') }}</dt>
          <dd>
            <a
              :href="app.website"
              target="_blank"
              rel="noopener noreferrer"
              class="text-primary hover:underline"
              >{{ t('appStore.show') }}</a
            >
          </dd>
        </div>
      </dl>

      <Alert v-if="!runsHere" variant="destructive" data-testid="app-architecture">
        {{ t('appStore.notForThisNasLong', { architectures: app.architectures.join(', ') }) }}
      </Alert>
      <Alert v-if="!app.official" class="border-amber-500/60">
        <ShieldAlert class="text-amber-600" />
        {{ t('appStore.unofficialHint') }}
      </Alert>

      <section class="grid gap-2 border-b pb-6">
        <h3 :class="heading">{{ t('appStore.description') }}</h3>
        <p
          v-for="(paragraph, index) in paragraphs(appText(app, 'description', locale))"
          :key="index"
          class="text-sm leading-relaxed text-muted-foreground"
        >
          {{ paragraph }}
        </p>
        <div
          v-if="notes.length"
          class="mt-2 grid gap-2 rounded-lg bg-amber-500/[0.07] p-4 text-sm leading-relaxed"
        >
          <p class="font-medium">{{ t('appStore.notice') }}</p>
          <p v-for="(paragraph, index) in notes" :key="index" class="text-muted-foreground">
            {{ paragraph }}
          </p>
        </div>
      </section>

      <section class="grid gap-2 border-b pb-6">
        <h3 :class="heading">{{ t('appStore.latest') }}</h3>
        <p class="flex flex-wrap gap-x-5 gap-y-1 text-sm">
          <span v-if="app.version">{{ t('appStore.versionIs', { version: app.version }) }}</span>
          <span v-else>{{ t('appStore.rollingHint') }}</span>
          <span class="text-muted-foreground">{{
            t('appStore.runsOn', { architectures: app.architectures.join(', ') })
          }}</span>
        </p>
      </section>

      <section class="grid gap-2 border-b pb-6 text-sm">
        <h3 :class="heading">{{ t('appStore.needs') }}</h3>
        <ul class="grid gap-1.5 text-muted-foreground">
          <li v-for="port in app.ports" :key="`port-${port.id}`">
            {{ t('appStore.port', { label: port.label, port: port.default }) }}
            <span v-if="port.fixed">({{ t('appStore.fixed') }})</span>
          </li>
          <li v-for="share in app.shares" :key="`share-${share.id}`">
            {{ t('appStore.share', { label: share.label }) }}
            <Badge variant="secondary">
              {{ share.read_write ? t('appStore.readWrite') : t('appStore.readOnly') }}
            </Badge>
            <span v-if="share.description" class="block text-xs">{{ share.description }}</span>
          </li>
          <li v-for="setting in asked" :key="`setting-${setting.id}`">
            {{ t('appStore.setting', { label: setting.label }) }}
          </li>
          <li v-if="generated.length">
            {{
              t(
                'appStore.generated',
                { labels: generated.map((s) => s.label).join(', ') },
                generated.length,
              )
            }}
          </li>
        </ul>
        <div v-if="asksMore" class="mt-2 grid gap-1.5 rounded-xl border border-amber-500/60 p-3">
          <p class="font-medium">{{ t('appStore.permissionsTitle') }}</p>
          <div class="text-muted-foreground"><PermissionList :permissions="app.permissions" /></div>
        </div>
      </section>

      <section class="grid gap-3 text-sm">
        <h3 :class="heading">{{ t('appStore.moreInfo') }}</h3>
        <div class="grid gap-2 @2xl:grid-cols-3">
          <a
            :href="app.website"
            target="_blank"
            rel="noopener noreferrer"
            class="text-primary hover:underline"
            >{{ t('appStore.website') }}</a
          >
          <a
            :href="app.source_code"
            target="_blank"
            rel="noopener noreferrer"
            class="text-primary hover:underline"
            >{{ t('appStore.sourceCode') }}</a
          >
          <button
            type="button"
            class="w-fit text-left text-primary hover:underline"
            @click="showCompose = !showCompose"
          >
            {{ showCompose ? t('appStore.hideCompose') : t('appStore.showCompose') }}
          </button>
        </div>
        <pre
          v-if="showCompose"
          class="max-h-80 overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs"
          >{{ app.compose }}</pre>
      </section>

      <InstallDialog v-model:open="installing" :app="app" @installed="emit('installed')" />
    </template>
  </div>
</template>
