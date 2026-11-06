export { default as TimeChart } from './TimeChart.vue'

export interface ChartSeries {
  label: string
  values: number[]
  /** A CSS colour, e.g. `var(--chart-1)`. */
  color: string
  /** Drawn dashed and without a filled area, for a second series. */
  dashed?: boolean
}
