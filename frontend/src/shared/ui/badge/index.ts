import { cva, type VariantProps } from 'class-variance-authority'

export { default as Badge } from './Badge.vue'

/**
 * What is fine is said quietly, in grey with a small green mark: colour is
 * kept for what is worth a look (warning) or wrong (destructive), so that
 * a page where all is well stays calm and the one thing that is not shows.
 */
export const badgeVariants = cva(
  'inline-flex h-5 w-fit shrink-0 items-center gap-1 rounded border px-1.5 text-2xs font-medium whitespace-nowrap [&>svg]:pointer-events-none [&>svg]:size-3',
  {
    variants: {
      variant: {
        default: 'border-transparent bg-primary/10 text-primary',
        secondary: 'border-transparent bg-secondary text-secondary-foreground',
        success:
          "border-transparent bg-transparent px-0 font-normal text-muted-foreground before:size-1.5 before:shrink-0 before:rounded-full before:bg-success before:content-[''] has-[>svg]:before:hidden [&>svg]:text-success",
        warning: 'border-transparent bg-amber-500/15 text-amber-700 dark:text-amber-400',
        destructive: 'border-transparent bg-destructive/15 text-destructive',
        outline: 'text-muted-foreground',
        info: 'border-transparent bg-info/15 text-info',
      },
    },
    defaultVariants: { variant: 'default' },
  },
)

export type BadgeVariants = VariantProps<typeof badgeVariants>
