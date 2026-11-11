import { computed } from 'vue'

import { useShareList } from '@/shared/ui/folder-picker'

import { useDrives } from './api'

/** The shares by id and by name, and the drives' labels: to name places. */
export function useNames() {
  const { data: shares } = useShareList()
  const { data: drives } = useDrives()
  const shareNames = computed(
    () => new Map((shares.value ?? []).map((share) => [share.id, share.name])),
  )
  const shareIds = computed(
    () => new Map((shares.value ?? []).map((share) => [share.name, share.id])),
  )
  const driveLabels = computed(
    () => new Map((drives.value ?? []).map((drive) => [drive.id, drive.label || drive.id])),
  )
  return { shares, drives, shareNames, shareIds, driveLabels }
}
