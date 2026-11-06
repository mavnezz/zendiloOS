export { default as StatTile } from './StatTile.vue'

export const statTones = {
  muted: 'border-border',
  primary: 'border-primary',
  success: 'border-success',
  warning: 'border-warning',
  destructive: 'border-destructive',
  'chart-1': 'border-chart-1',
  'chart-2': 'border-chart-2',
  'chart-3': 'border-chart-3',
  'chart-4': 'border-chart-4',
  'chart-5': 'border-chart-5',
} as const

export type StatTone = keyof typeof statTones
