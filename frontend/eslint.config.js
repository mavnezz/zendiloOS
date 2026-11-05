import { readdirSync } from 'node:fs'

import skipFormatting from '@vue/eslint-config-prettier/skip-formatting'
import { defineConfigWithVueTs, vueTsConfigs } from '@vue/eslint-config-typescript'
import { createTypeScriptImportResolver } from 'eslint-import-resolver-typescript'
import { importX } from 'eslint-plugin-import-x'
import pluginVue from 'eslint-plugin-vue'

// Dependencies point inward: app → desktop → features → shared.
// Features do not import each other (docs/architecture.md, 2.2).
const features = readdirSync('src/features', { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name)

const zones = [
  {
    target: './src/shared',
    from: ['./src/features', './src/desktop', './src/app'],
    message: 'shared/ must not depend on features, the desktop or the app shell.',
  },
  {
    target: './src/features',
    from: ['./src/desktop', './src/app'],
    message: 'Features know nothing about the desktop or the app shell.',
  },
  {
    target: './src/desktop',
    from: './src/app',
    message: 'The desktop must not depend on the app shell.',
  },
  ...features.map((feature) => ({
    target: `./src/features/${feature}`,
    from: './src/features',
    except: [`./${feature}`],
    message: 'Features must not import each other; move shared code to shared/.',
  })),
]

export default defineConfigWithVueTs(
  {
    name: 'zendiloos/ignores',
    ignores: ['dist/', 'public/', 'coverage/', 'src/shared/api/schema.d.ts'],
  },
  pluginVue.configs['flat/recommended'],
  vueTsConfigs.recommendedTypeChecked,
  {
    name: 'zendiloos/vue',
    rules: {
      // Optional props in TypeScript are explicitly `undefined`; a default adds nothing.
      'vue/require-default-prop': 'off',
    },
  },
  {
    // UI building blocks deliberately have short names, like the elements they wrap.
    name: 'zendiloos/ui-names',
    files: ['src/shared/ui/**/*.vue'],
    rules: { 'vue/multi-word-component-names': 'off' },
  },
  {
    name: 'zendiloos/boundaries',
    files: ['src/**/*.{ts,vue}'],
    plugins: { 'import-x': importX },
    settings: {
      'import-x/resolver-next': [
        createTypeScriptImportResolver({ project: './tsconfig.app.json' }),
      ],
    },
    rules: {
      'import-x/no-restricted-paths': ['error', { zones }],
    },
  },
  skipFormatting,
)
