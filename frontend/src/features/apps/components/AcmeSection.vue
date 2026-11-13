<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import {
  FileBadge,
  KeyRound,
  LoaderCircle,
  Pencil,
  Plus,
  RefreshCw,
  ScrollText,
  Trash2,
  UserRound,
} from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AcmeAccount, AcmeChallenge, Certificate } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatDateTime } from '@/shared/lib/format'
import { ActionMenu, type Action } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { PanelSection } from '@/shared/ui/section'

import {
  holds,
  installAcme,
  useAcmeAccounts,
  useAcmeActions,
  useAcmeChallenges,
  useAcmePlugin,
  useCertificates,
} from '../acme'
import AcmeAccountDialog from './AcmeAccountDialog.vue'
import AcmeCertificateDialog from './AcmeCertificateDialog.vue'
import AcmeChallengeDialog from './AcmeChallengeDialog.vue'
import AcmeLogDialog from './AcmeLogDialog.vue'

const { t } = useI18n()
const client = useQueryClient()
const jobs = useJobsStore()
const actions = useAcmeActions()
const { data: plugin, error } = useAcmePlugin()
const installed = computed(() => plugin.value?.installed === true)
const { data: accounts } = useAcmeAccounts(() => installed.value)
const { data: challenges } = useAcmeChallenges(() => installed.value)
const { data: certificates } = useCertificates(() => installed.value)
const failure = ref<string | null>(null)

/** The latest installation of acme.sh, running or done. */
const installJob = computed(() => jobs.list.find((job) => job.kind === 'certificates.acme_install'))
const installing = computed(() => installJob.value?.state === 'running')
const installFailed = computed(() =>
  installJob.value?.state === 'failed' ? installJob.value : null,
)
const issuing = computed(
  () =>
    new Set(
      jobs.list
        .filter((job) => job.kind === 'certificates.issue' && job.state === 'running')
        .map((job) => job.target),
    ),
)

// Once a run ends: what acme.sh and the certificates are now.
watch(
  () =>
    jobs.list.filter((job) => job.kind.startsWith('certificates.') && job.state !== 'running')
      .length,
  () => void client.invalidateQueries({ queryKey: ['certificates'] }),
)

async function run(work: () => Promise<unknown>): Promise<void> {
  failure.value = null
  try {
    await work()
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}

const install = () => run(async () => jobs.update(await installAcme()))

// Dialogs: one each for accounts, challenges and certificates, and the log.
const editingAccount = ref<AcmeAccount | null>(null)
const accountOpen = ref(false)
const editingChallenge = ref<AcmeChallenge | null>(null)
const challengeOpen = ref(false)
const editingCertificate = ref<Certificate | null>(null)
const certificateOpen = ref(false)
const logCertificate = ref<Certificate | null>(null)
const logJob = ref<number | null>(null)
const logOpen = ref(false)
const deleting = ref<{
  kind: 'account' | 'challenge' | 'certificate'
  id: number
  name: string
} | null>(null)
const deleteOpen = ref(false)

function editAccount(account: AcmeAccount | null): void {
  editingAccount.value = account
  accountOpen.value = true
}
function editChallenge(challenge: AcmeChallenge | null): void {
  editingChallenge.value = challenge
  challengeOpen.value = true
}
function editCertificate(certificate: Certificate | null): void {
  editingCertificate.value = certificate
  certificateOpen.value = true
}
function showLog(certificate: Certificate, job: number | null = null): void {
  logCertificate.value = certificate
  logJob.value = job
  logOpen.value = true
}
function issue(certificate: Certificate): Promise<void> {
  return run(async () => {
    const job = await actions.issue(certificate.id)
    jobs.update(job)
    showLog(certificate, job.id)
  })
}
/** A new certificate is issued at once. */
function created(certificate: Certificate | null): void {
  if (certificate) void issue(certificate)
}
function askDelete(kind: 'account' | 'challenge' | 'certificate', id: number, name: string): void {
  deleting.value = { kind, id, name }
  deleteOpen.value = true
}
function remove(): Promise<void> {
  const target = deleting.value
  if (!target) return Promise.resolve()
  return run(() =>
    target.kind === 'account'
      ? actions.deleteAccount(target.id)
      : target.kind === 'challenge'
        ? actions.deleteChallenge(target.id)
        : actions.deleteCertificate(target.id),
  )
}

const date = (seconds: number) => formatDateTime(seconds * 1000)
const accountName = (id: number) => accounts.value?.find((a) => a.id === id)?.name ?? '–'
const challengeName = (id: number) => challenges.value?.find((c) => c.id === id)?.name ?? '–'

function state(certificate: Certificate): {
  text: string
  variant: 'success' | 'secondary' | 'destructive' | 'warning'
} {
  if (holds(certificate)) {
    const text = t('appStore.acme.validUntil', { date: date(certificate.not_after ?? 0) })
    return { text, variant: certificate.last_error ? 'warning' : 'success' }
  }
  if (certificate.not_after)
    return {
      text: t('appStore.acme.expired', { date: date(certificate.not_after) }),
      variant: 'destructive',
    }
  return certificate.last_error
    ? { text: t('appStore.acme.lastFailed'), variant: 'destructive' }
    : { text: t('appStore.acme.notIssued'), variant: 'secondary' }
}

function certificateActions(certificate: Certificate): Action[] {
  return [
    {
      id: 'issue',
      label: certificate.issued_at ? t('appStore.acme.renew') : t('appStore.acme.issue'),
      icon: RefreshCw,
      disabled: issuing.value.has(certificate.domains[0] ?? ''),
    },
    { id: 'log', label: t('appStore.acme.log'), icon: ScrollText },
    { id: 'edit', label: t('appStore.acme.edit'), icon: Pencil },
    { id: 'delete', label: t('appStore.acme.delete'), icon: Trash2, destructive: true },
  ]
}
function onCertificate(certificate: Certificate, action: string): void {
  if (action === 'issue') void issue(certificate)
  else if (action === 'log') showLog(certificate)
  else if (action === 'edit') editCertificate(certificate)
  else askDelete('certificate', certificate.id, certificate.domains[0] ?? '')
}
const rowActions = (): Action[] => [
  { id: 'edit', label: t('appStore.acme.edit'), icon: Pencil },
  { id: 'delete', label: t('appStore.acme.delete'), icon: Trash2, destructive: true },
]

const ready = computed(() => Boolean(accounts.value?.length && challenges.value?.length))
const fact = 'grid gap-1 px-3 text-center not-first:border-l'
const heading = 'text-base font-semibold'
const row = 'flex items-center gap-3 rounded-lg border bg-card px-3 py-2.5'
</script>

<!--
  The ACME client plugin: acme.sh from Debian, its accounts at Let's
  Encrypt, the challenges that show a name is ours, and the certificates,
  issued in jobs whose log is what acme.sh writes.
-->
<template>
  <div class="grid gap-6">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <header class="flex flex-wrap items-center gap-5 px-2">
      <span
        class="grid size-20 shrink-0 place-items-center rounded-tile bg-linear-to-br from-emerald-400 to-teal-600 text-white shadow-sm"
      >
        <FileBadge class="size-10" />
      </span>
      <div class="grid min-w-0 flex-1 gap-1">
        <h2 class="text-xl font-semibold">{{ t('appStore.acme.name') }}</h2>
        <p class="text-sm text-muted-foreground">{{ t('appStore.acme.subtitle') }}</p>
      </div>
      <span v-if="installing" class="flex items-center gap-2 text-sm text-muted-foreground">
        <LoaderCircle class="size-4 animate-spin" />
        {{ t('appStore.acme.installing') }}
      </span>
      <Button
        v-else-if="plugin && !installed"
        size="lg"
        class="rounded-full px-6"
        @click="install"
        >{{ t('appStore.install') }}</Button
      >
    </header>

    <dl class="grid grid-cols-3 gap-y-4 border-y py-4 text-sm">
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.docker.status') }}</dt>
        <dd class="font-medium">
          {{
            plugin
              ? installed
                ? t('appStore.acme.installed')
                : t('appStore.acme.notInstalled')
              : '–'
          }}
        </dd>
      </div>
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.acme.certificates') }}</dt>
        <dd>{{ installed ? (certificates?.length ?? '–') : '–' }}</dd>
      </div>
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.docker.source') }}</dt>
        <dd>Debian (acme.sh)</dd>
      </div>
    </dl>

    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <div v-if="installFailed && !installed" class="grid gap-2">
      <Alert variant="destructive">{{ t('appStore.acme.failed') }}</Alert>
      <pre
        v-if="installFailed.error_detail"
        class="max-h-60 overflow-auto rounded-md bg-zinc-900 p-3 font-mono text-xs whitespace-pre-wrap text-zinc-100"
        >{{ installFailed.error_detail }}</pre>
    </div>

    <template v-if="installed">
      <PanelSection :title="t('appStore.acme.certificates')">
        <template #actions>
          <Button size="sm" variant="secondary" :disabled="!ready" @click="editCertificate(null)">
            <Plus />
            {{ t('appStore.acme.add') }}
          </Button>
        </template>
        <p v-if="!ready" class="text-sm text-muted-foreground">
          {{ t('appStore.acme.firstAccount') }}
        </p>
        <p v-else-if="!certificates?.length" class="text-sm text-muted-foreground">
          {{ t('appStore.acme.noCertificates') }}
        </p>
        <ul class="grid gap-2">
          <li v-for="certificate in certificates ?? []" :key="certificate.id" :class="row">
            <FileBadge class="size-5 shrink-0 text-muted-foreground" />
            <div class="grid min-w-0 flex-1 gap-0.5">
              <span class="flex flex-wrap items-center gap-2">
                <span class="truncate font-medium">{{ certificate.domains[0] }}</span>
                <span v-if="certificate.domains.length > 1" class="text-xs text-muted-foreground"
                  >+{{ certificate.domains.length - 1 }}</span
                >
                <Badge v-if="issuing.has(certificate.domains[0] ?? '')" variant="warning">
                  <LoaderCircle class="size-3 animate-spin" />
                  {{ t('appStore.acme.logRunning') }}
                </Badge>
                <Badge v-else :variant="state(certificate).variant">{{
                  state(certificate).text
                }}</Badge>
              </span>
              <span class="truncate text-xs text-muted-foreground">
                {{ accountName(certificate.account_id) }} ·
                {{ challengeName(certificate.challenge_id) }} ·
                {{ t(`appStore.acme.keys.${certificate.key}`) }} ·
                {{
                  certificate.auto_renew ? t('appStore.acme.autoRenew') : t('appStore.acme.manual')
                }}
              </span>
              <span
                v-if="certificate.last_error"
                class="truncate font-mono text-xs text-destructive"
                >{{ certificate.last_error }}</span
              >
            </div>
            <ActionMenu
              :actions="certificateActions(certificate)"
              :label="certificate.domains[0] ?? ''"
              @select="(action) => onCertificate(certificate, action)"
            />
          </li>
        </ul>
      </PanelSection>

      <div class="grid gap-6 @3xl:grid-cols-2">
        <PanelSection :title="t('appStore.acme.accounts')">
          <template #actions>
            <Button size="sm" variant="secondary" @click="editAccount(null)">
              <Plus />
              {{ t('appStore.acme.add') }}
            </Button>
          </template>
          <p v-if="!accounts?.length" class="text-sm text-muted-foreground">
            {{ t('appStore.acme.noAccounts') }}
          </p>
          <ul class="grid gap-2">
            <li v-for="account in accounts ?? []" :key="account.id" :class="row">
              <UserRound class="size-5 shrink-0 text-muted-foreground" />
              <div class="grid min-w-0 flex-1">
                <span class="truncate font-medium">{{ account.name }}</span>
                <span class="truncate text-xs text-muted-foreground"
                  >{{ account.email }} · {{ t(`appStore.acme.servers.${account.server}`) }}</span
                >
              </div>
              <ActionMenu
                :actions="rowActions()"
                :label="account.name"
                @select="
                  (action) =>
                    action === 'edit'
                      ? editAccount(account)
                      : askDelete('account', account.id, account.name)
                "
              />
            </li>
          </ul>
        </PanelSection>

        <PanelSection :title="t('appStore.acme.challenges')">
          <template #actions>
            <Button size="sm" variant="secondary" @click="editChallenge(null)">
              <Plus />
              {{ t('appStore.acme.add') }}
            </Button>
          </template>
          <p v-if="!challenges?.length" class="text-sm text-muted-foreground">
            {{ t('appStore.acme.noChallenges') }}
          </p>
          <ul class="grid gap-2">
            <li v-for="challenge in challenges ?? []" :key="challenge.id" :class="row">
              <KeyRound class="size-5 shrink-0 text-muted-foreground" />
              <div class="grid min-w-0 flex-1">
                <span class="truncate font-medium">{{ challenge.name }}</span>
                <span class="truncate text-xs text-muted-foreground">
                  {{ t(`appStore.acme.methods.${challenge.method}`) }}
                  <template v-if="challenge.provider">
                    ·
                    {{
                      plugin?.providers.find((p) => p.api === challenge.provider)?.label ??
                      challenge.provider
                    }}
                  </template>
                </span>
              </div>
              <ActionMenu
                :actions="rowActions()"
                :label="challenge.name"
                @select="
                  (action) =>
                    action === 'edit'
                      ? editChallenge(challenge)
                      : askDelete('challenge', challenge.id, challenge.name)
                "
              />
            </li>
          </ul>
        </PanelSection>
      </div>
    </template>

    <section class="grid gap-2 border-b pb-6">
      <h3 :class="heading">{{ t('appStore.description') }}</h3>
      <p class="text-sm leading-relaxed text-muted-foreground">{{ t('appStore.acme.about') }}</p>
      <p class="text-sm leading-relaxed text-muted-foreground">{{ t('appStore.acme.aboutUse') }}</p>
    </section>
    <section class="grid gap-2 text-sm">
      <h3 :class="heading">{{ t('appStore.needs') }}</h3>
      <ul class="grid list-disc gap-1.5 pl-5 text-muted-foreground">
        <li>{{ t('appStore.acme.needsPackages') }}</li>
        <li>{{ t('appStore.acme.needsName') }}</li>
        <li>{{ t('appStore.acme.needsNetwork') }}</li>
      </ul>
    </section>

    <AcmeAccountDialog v-model:open="accountOpen" :account="editingAccount" />
    <AcmeChallengeDialog
      v-model:open="challengeOpen"
      :challenge="editingChallenge"
      :providers="plugin?.providers ?? []"
    />
    <AcmeCertificateDialog
      v-model:open="certificateOpen"
      :certificate="editingCertificate"
      :accounts="accounts ?? []"
      :challenges="challenges ?? []"
      @saved="created"
    />
    <AcmeLogDialog v-model:open="logOpen" :certificate="logCertificate" :job-id="logJob" />
    <ConfirmDialog
      v-model:open="deleteOpen"
      destructive
      :title="
        deleting ? t(`appStore.acme.confirmDelete.${deleting.kind}`, { name: deleting.name }) : ''
      "
      :description="t('appStore.acme.confirmDelete.hint')"
      :confirm-label="t('appStore.acme.delete')"
      :cancel-label="t('desktop.cancel')"
      @confirm="remove"
    />
  </div>
</template>
