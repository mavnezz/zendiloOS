import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { isAbsolute, join, resolve } from 'node:path'
import { fileURLToPath, URL } from 'node:url'

import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import type { Plugin } from 'vite'
import { defineConfig } from 'vitest/config'

/** A package of npm's whose code or styles are in the bundle. */
interface Bundled {
  name: string
  version: string
  license: string
  repository: string | null
}

/**
 * Writes beside the bundle what it carries of npm's packages: each one
 * whose modules the build took in (not those only the tools use, nor parts
 * of a package nothing imports), in `licences.json` with its licence, which
 * the licence overview and the SBOM are made from (scripts/licences.mjs,
 * scripts/sbom.sh), and their licence texts in `third-party-licences.txt`,
 * which their licences ask to go along with what is distributed.
 */
function bundledPackages(): Plugin {
  return {
    name: 'zendiloos-bundled-packages',
    apply: 'build',
    generateBundle(_, bundle) {
      const found = new Map<string, Bundled & { text: string }>()
      // The modules taken in, and what assets came from (the fonts a
      // style sheet imports are no modules of the graph).
      const sources = [...this.getModuleIds()]
      for (const output of Object.values(bundle)) {
        if (output.type !== 'asset') continue
        // Named from the project's folder.
        const here = fileURLToPath(new URL('.', import.meta.url))
        sources.push(
          ...output.originalFileNames.map((name) =>
            isAbsolute(name) ? name : resolve(here, name),
          ),
        )
      }
      for (const id of sources) {
        const path = id.split('?')[0]
        const at = path.lastIndexOf('/node_modules/')
        if (id.startsWith('\0') || at < 0) continue
        const rest = path.slice(at + '/node_modules/'.length).split('/')
        const name = rest[0].startsWith('@') ? `${rest[0]}/${rest[1]}` : rest[0]
        const root = path.slice(0, at + '/node_modules/'.length) + name
        if (found.has(root) || !existsSync(join(root, 'package.json'))) continue
        const manifest = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')) as {
          name: string
          version: string
          license?: string
          repository?: string | { url?: string }
        }
        const repository =
          typeof manifest.repository === 'string' ? manifest.repository : manifest.repository?.url
        const file = readdirSync(root).find((entry) => /^(licen[cs]e|copying)(\.|$)/i.test(entry))
        found.set(root, {
          name: manifest.name,
          version: manifest.version,
          license: manifest.license ?? 'NOASSERTION',
          repository: repository ?? null,
          text: file && existsSync(join(root, file)) ? readFileSync(join(root, file), 'utf8') : '',
        })
      }
      // A package in two places of node_modules counts once.
      const unique = new Map([...found.values()].map((p) => [`${p.name}@${p.version}`, p]))
      const packages = [...unique.values()].sort(
        (a, b) => a.name.localeCompare(b.name, 'en') || a.version.localeCompare(b.version, 'en'),
      )
      const list: Bundled[] = packages.map(({ name, version, license, repository }) => ({
        name,
        version,
        license,
        repository,
      }))
      this.emitFile({
        type: 'asset',
        fileName: 'licences.json',
        source: `${JSON.stringify(list, null, 2)}\n`,
      })
      const texts = packages.map(
        (p) =>
          `${p.name} ${p.version} (${p.license})\n${'='.repeat(72)}\n\n${p.text.trim() || 'No licence text in the package.'}\n`,
      )
      this.emitFile({
        type: 'asset',
        fileName: 'third-party-licences.txt',
        source: `What the web interface bundles of npm's packages, with their licences.\n\n${texts.join('\n\n')}`,
      })
    },
  }
}

export default defineConfig({
  plugins: [vue(), tailwindcss(), bundledPackages()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  server: {
    port: 5173,
    strictPort: true,
    // `just dev` serves the API here; the browser sees one origin, as with nginx.
    proxy: { '/api': { target: 'http://127.0.0.1:8780', ws: true } },
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts'],
    // `npm run test:coverage` (just coverage-web); CI holds the thresholds.
    coverage: {
      provider: 'v8',
      include: ['src/**/*.{ts,vue}'],
      // The tests' own helpers (stand-in server, mounting) are not measured.
      exclude: [
        'src/**/*.test.ts',
        'src/**/*.d.ts',
        'src/main.ts',
        'src/shared/testing/**',
        'src/shared/test-support/**',
      ],
      reporter: ['text-summary', 'html', 'json-summary'],
      reportsDirectory: '../dev/coverage/frontend',
      // Just below what the tests reach (11/2026: lines 94 %, statements
      // 93 %, functions 89 %, branches 85 %, .ts lines 97 %): coverage may
      // rise, not fall unnoticed. The logic in .ts files is held higher.
      thresholds: {
        lines: 93,
        statements: 92,
        functions: 88,
        branches: 84,
        'src/**/*.ts': { lines: 96 },
      },
    },
  },
})
