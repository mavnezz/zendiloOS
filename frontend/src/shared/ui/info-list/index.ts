export { default as InfoList } from './InfoList.vue'

export interface InfoItem {
  label: string
  value: string
  /** A technical value: device names, addresses, versions. */
  mono?: boolean
}
