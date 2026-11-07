import type { ComputedRef, InjectionKey } from 'vue'

import type { Item } from './actions'
import type { Location, Place } from './location'

/** What the explorer lends its tree: where it is, and how to act there. */
export interface Explorer {
  current: ComputedRef<Location>
  go(location: Location): void
  /** A right click (or long press) on a node: the menu is about `item`. */
  aim(item: Item): void
  /** Something dropped on the folder `share` / `path`. */
  drop(event: DragEvent, share: Place, path: string): void
}

export const EXPLORER: InjectionKey<Explorer> = Symbol('explorer')

/** The item a folder of the tree stands for. */
export function folderItem(
  share: Place,
  path: string,
  name: string,
  extra: Partial<Item> = {},
): Item {
  return {
    key: `${share}:${path}`,
    name,
    kind: 'directory',
    size: null,
    modified: null,
    unreadable: false,
    share,
    path,
    ...extra,
  }
}
