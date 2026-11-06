export { default as IconTile } from './IconTile.vue'

/** Colours of icon tiles, top to bottom of their gradient. */
export const tileTones = {
  blue: 'from-sky-400 to-blue-600',
  indigo: 'from-indigo-400 to-indigo-600',
  violet: 'from-violet-400 to-purple-600',
  teal: 'from-teal-400 to-teal-600',
  green: 'from-emerald-400 to-green-600',
  amber: 'from-amber-300 to-orange-500',
  rose: 'from-rose-400 to-red-600',
  slate: 'from-slate-500 to-slate-700',
  graphite: 'from-zinc-600 to-zinc-800',
  sky: 'from-cyan-400 to-sky-600',
} as const

export type TileTone = keyof typeof tileTones

export const tileSizes = {
  xs: { tile: 'size-4 rounded-[5px]', icon: 'size-2.5', stroke: 2.5 },
  sm: { tile: 'size-5 rounded-[6px]', icon: 'size-3', stroke: 2.25 },
  md: { tile: 'size-7 rounded-lg', icon: 'size-4', stroke: 2 },
  lg: { tile: 'size-10 rounded-[11px]', icon: 'size-5', stroke: 2 },
  xl: { tile: 'size-12 rounded-[13px]', icon: 'size-6', stroke: 1.75 },
} as const
