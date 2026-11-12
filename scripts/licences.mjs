#!/usr/bin/env node
// The overview of the licences zendiloOS's package carries, docs/licences.md,
// made from the software bill of materials (scripts/sbom.sh sources), and
// its check (`just licences-check`, part of `just check`): every licence of
// a crate or of a package in the web interface's bundle is one that
// backend/deny.toml allows, or an exception named here for one package; and
// docs/licences.md says what the sources bring now.
//
// Usage: node scripts/licences.mjs SBOM            prints the overview
//        node scripts/licences.mjs --check SBOM    checks both, quietly

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')

/**
 * Licences beyond deny.toml's, each for one package of the web interface
 * and decided here, as cargo-deny's exceptions are for one crate.
 */
const EXCEPTIONS = {
  '@novnc/novnc': {
    licence: 'MPL-2.0',
    why: 'noVNC, the console of virtual machines, bundled unmodified: the MPL binds its own files only, whose source is published (debian/copyright names it)',
  },
  '@fontsource-variable/geist': {
    licence: 'OFL-1.1',
    why: "the interface's font: the OFL lets fonts go with software, its text goes along",
  },
  '@fontsource-variable/jetbrains-mono': {
    licence: 'OFL-1.1',
    why: "the interface's monospaced font, likewise",
  },
}

/** The licences backend/deny.toml allows: `allow = [ … ]` of `[licenses]`. */
function allowedLicences() {
  const text = readFileSync(join(root, 'backend/deny.toml'), 'utf8')
  const section = text.split(/^\[licenses\]$/m)[1]?.split(/^\[/m)[0] ?? ''
  const list = section.match(/^allow\s*=\s*\[([\s\S]*?)^\]/m)?.[1] ?? ''
  const names = list
    .split('\n')
    .map((line) => line.replace(/#.*/, ''))
    .flatMap((line) => [...line.matchAll(/"([^"]+)"/g)].map((match) => match[1]))
  if (names.length === 0) throw new Error('backend/deny.toml allows no licences')
  return new Set(names)
}

/**
 * Whether an SPDX expression is met by the licences `allowed`: one side of
 * an OR, both of an AND; `A WITH exception` where it is allowed so, or A
 * is. The old `/` counts as OR.
 */
export function satisfied(expression, allowed) {
  const tokens = expression
    .replaceAll('(', ' ( ')
    .replaceAll(')', ' ) ')
    .replaceAll('/', ' OR ')
    .split(/\s+/)
    .filter(Boolean)
  let at = 0
  const peek = () => tokens[at]
  const next = () => tokens[at++]
  const atom = () => {
    if (peek() === '(') {
      next()
      const inner = or()
      if (next() !== ')') throw new Error(`unbalanced: ${expression}`)
      return inner
    }
    const id = next()
    if (id === undefined) throw new Error(`incomplete: ${expression}`)
    if (peek() === 'WITH') {
      next()
      const exception = next()
      return allowed.has(`${id} WITH ${exception}`) || allowed.has(id)
    }
    return allowed.has(id)
  }
  const and = () => {
    let met = atom()
    while (peek() === 'AND') {
      next()
      met = atom() && met
    }
    return met
  }
  const or = () => {
    let met = and()
    while (peek() === 'OR') {
      next()
      met = and() || met
    }
    return met
  }
  const met = or()
  if (at !== tokens.length) throw new Error(`not an expression: ${expression}`)
  return met
}

const part = (component) =>
  (component.properties ?? []).find((property) => property.name === 'zendiloos:part')?.value
const licence = (component) =>
  (component.licenses ?? [])
    .map((entry) => entry.expression ?? entry.license?.id ?? entry.license?.name)
    .filter(Boolean)
    .join(' AND ') || 'NOASSERTION'

/** What the package asks of Debian, from debian/control: by relation. */
function debianNeeds() {
  const control = readFileSync(join(root, 'debian/control'), 'utf8')
  const stanza = control.split(/\n\s*\n/).find((block) => /^Package: zendiloos$/m.test(block)) ?? ''
  const field = (name) =>
    (stanza.match(new RegExp(`^${name}:([^\\n]*(?:\\n[ \\t][^\\n]*)*)`, 'm'))?.[1] ?? '')
      .split(',')
      .map((need) => need.trim())
      .filter((need) => need && !need.startsWith('${'))
  return { Depends: field('Depends'), Recommends: field('Recommends'), Suggests: field('Suggests') }
}

/** Rows of `| licence | names |`, by licence. */
function table(components, heading) {
  const byLicence = new Map()
  for (const component of components) {
    const key = licence(component)
    byLicence.set(key, [...(byLicence.get(key) ?? []), `${component.name} ${component.version}`])
  }
  const rows = [...byLicence.entries()]
    .sort(([a], [b]) => a.localeCompare(b, 'en'))
    .map(([key, names]) => `| ${key} | ${names.length} | ${names.sort((a, b) => a.localeCompare(b, 'en')).join(', ')} |`)
  return [`| Licence | ${heading} | Which |`, '| --- | --- | --- |', ...rows].join('\n')
}

function overview(sbom, allowed) {
  const components = sbom.components ?? []
  const own = (component) => component.name.startsWith('zendiloos')
  const crates = components.filter((c) => part(c) === 'programs' && !own(c))
  const ownCrates = components.filter((c) => part(c) === 'programs' && own(c))
  const web = components.filter((c) => part(c) === 'web interface')
  const debian = debianNeeds()
  const exceptions = Object.entries(EXCEPTIONS)
    .map(([name, { licence: id, why }]) => `| ${name} | ${id} | ${why} |`)
    .join('\n')
  return `# Licences

<!-- Made by \`just licences\` from the software bill of materials; do not edit. -->

zendiloOS is under the Apache License 2.0. This page lists what its package
carries of others' work and under which licences. It is made from the
software bill of materials (\`scripts/sbom.sh\`), the same one every release
publishes beside its package, and \`just check\` holds both to the rules:
every licence is one \`backend/deny.toml\` allows, or an exception below for
one package of the web interface.

Allowed: ${[...allowed].sort((a, b) => a.localeCompare(b, 'en')).join(', ')}.

| Exception | Licence | Why |
| --- | --- | --- |
${exceptions}

## The programs

zendiloosd and zendiloos-agent, built for x86-64 Linux, hold ${crates.length} crates of
others (build scripts and what only they use aside), and ${ownCrates.length} of zendiloOS's
own (Apache-2.0).

${table(crates, 'Crates')}

## The web interface

Its bundle holds ${web.length} packages from npm, as the build found them in it
(\`frontend/dist/licences.json\`; the tools that only build it are not
there). Their licence texts go along with it: on the NAS in
\`/usr/share/zendiloos/webui/third-party-licences.txt\`.

${table(web, 'Packages')}

## Debian's packages

The package asks these of Debian 13, which installs them from its mirror
with their licences (\`/usr/share/doc/<package>/copyright\` on the NAS);
besides, the libraries the programs link (\`\${shlibs:Depends}\`).

| Relation | Packages |
| --- | --- |
${Object.entries(debian)
  .map(([relation, needs]) => `| ${relation} | ${needs.join(', ')} |`)
  .join('\n')}
`
}

/** What breaks the rules: a licence deny.toml does not allow, and no exception. */
function breaches(sbom, allowed) {
  return (sbom.components ?? [])
    .filter((component) => ['programs', 'web interface'].includes(part(component)))
    .filter((component) => {
      const id = licence(component)
      const exception = EXCEPTIONS[component.name]
      if (exception && part(component) === 'web interface') {
        return !satisfied(id, new Set([...allowed, exception.licence]))
      }
      return !satisfied(id, allowed)
    })
    .map((component) => `${component.name} ${component.version}: ${licence(component)}`)
}

function main(args) {
  const check = args[0] === '--check'
  const file = check ? args[1] : args[0]
  if (!file) {
    console.error('usage: node scripts/licences.mjs [--check] SBOM')
    process.exit(2)
  }
  const sbom = JSON.parse(readFileSync(file, 'utf8'))
  const allowed = allowedLicences()
  const text = overview(sbom, allowed)
  if (!check) {
    process.stdout.write(text)
    return
  }
  let failed = false
  for (const breach of breaches(sbom, allowed)) {
    console.error(`licences: ${breach}, which backend/deny.toml does not allow`)
    failed = true
  }
  if (readFileSync(join(root, 'docs/licences.md'), 'utf8') !== text) {
    console.error('licences: docs/licences.md is out of date; `just licences` writes it anew')
    failed = true
  }
  process.exit(failed ? 1 : 0)
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main(process.argv.slice(2))
