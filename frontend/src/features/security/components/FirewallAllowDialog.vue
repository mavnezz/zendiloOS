<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { RadioGroup, RadioGroupItem } from '@/shared/ui/radio-group'

import { sourceValid } from '../lib/firewall'

/** The service's name as shown, and the networks the NAS is in. */
const props = defineProps<{ name: string; localNetworks: string[]; busy?: boolean }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ allow: [sources: string[]] }>()

const { t } = useI18n()
const from = ref('local')
const custom = ref('')

watch(open, (isOpen) => {
  if (!isOpen) return
  from.value = props.localNetworks.length ? 'local' : 'any'
  custom.value = ''
})

const sources = computed(() => {
  if (from.value === 'local') return props.localNetworks
  if (from.value === 'any') return ['any']
  return [custom.value.trim()]
})
const valid = computed(
  () => from.value !== 'custom' || (custom.value.trim() !== '' && sourceValid(custom.value)),
)
</script>

<!-- Lets a blocked service through: a rule at the top of the list, from whom it says. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('firewall.blocked.allowTitle', { service: name })"
    :description="t('firewall.blocked.allowHint')"
    :close-label="t('desktop.close')"
  >
    <form id="firewall-allow" class="grid gap-3" @submit.prevent="valid && emit('allow', sources)">
      <RadioGroup v-model="from" :label="t('firewall.blocked.from')">
        <RadioGroupItem v-if="localNetworks.length" value="local">
          {{ t('firewall.blocked.local', { networks: localNetworks.join(', ') }) }}
        </RadioGroupItem>
        <RadioGroupItem value="any">{{ t('firewall.anySource') }}</RadioGroupItem>
        <RadioGroupItem value="custom">{{ t('firewall.rule.sourceCustom') }}</RadioGroupItem>
      </RadioGroup>
      <Input
        v-if="from === 'custom'"
        v-model="custom"
        class="font-mono text-[13px]"
        placeholder="192.168.1.0/24"
        :aria-label="t('firewall.rule.sourceCustom')"
        :aria-invalid="!!custom && !sourceValid(custom)"
      />
    </form>
    <template #footer>
      <Button type="button" variant="secondary" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button type="submit" form="firewall-allow" :disabled="!valid || busy">{{
        t('firewall.blocked.allow')
      }}</Button>
    </template>
  </ModalDialog>
</template>
