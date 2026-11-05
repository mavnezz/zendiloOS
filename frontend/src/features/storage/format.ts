import { i18n } from '@/shared/i18n'

/** `raid1` → `RAID 1`, `linear` → `Linear`. */
export function formatRaidLevel(level: string | null | undefined): string {
  if (!level) return i18n.global.t('storage.unknownLevel')
  const raid = /^raid(\d+)$/.exec(level)
  if (raid) return `RAID ${raid[1]}`
  return level.charAt(0).toUpperCase() + level.slice(1)
}

export function formatCount(value: number): string {
  return new Intl.NumberFormat(i18n.global.locale.value).format(value)
}
