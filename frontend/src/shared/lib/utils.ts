import { type ClassValue, clsx } from 'clsx'
import { extendTailwindMerge } from 'tailwind-merge'

// The sizes and corners global.css adds to Tailwind's: without them here,
// `text-dense` would be taken for a colour and dropped beside one.
const twMerge = extendTailwindMerge({
  extend: {
    theme: {
      text: ['dense', '2xs', '3xs'],
      radius: ['window', 'panel', 'tile'],
    },
  },
})

/** Joins class names; later Tailwind classes override earlier ones. */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs))
}
