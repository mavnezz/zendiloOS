// Names for the generated schema types used across the frontend.
import type { components } from './schema'

type Schemas = components['schemas']

export type SessionInfo = Schemas['SessionInfo']
export type SetupRequest = Schemas['SetupRequest']
export type SystemInfo = Schemas['SystemInfo']
export type PowerAction = Schemas['PowerActionKind']
export type Metrics = Schemas['Metrics']
export type LiveTopic = Schemas['LiveTopic']
export type LiveRequest = Schemas['LiveRequest']
export type LiveMessage = Schemas['LiveMessage']
