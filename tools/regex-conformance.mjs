#!/usr/bin/env node
/**
 * Dev-only regex conformance helper.
 *
 * Compares a small set of Oniguruma patterns against vscode-oniguruma (oracle)
 * and the mark-syntax regex-parse example (in-house hybrid matcher).
 *
 * Requires:
 *   npm install --prefix tools/golden-oracle
 *   cargo (for the mark-syntax example)
 *
 * Not used by release builds.
 */
import fs from 'node:fs/promises'
import path from 'node:path'
import process from 'node:process'
import { createRequire } from 'node:module'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { spawnSync } from 'node:child_process'

const out = arg('--out') ?? 'target/regex-conformance-phase2.json'
const require = createRequire(import.meta.url)
const toolDir = path.dirname(fileURLToPath(import.meta.url))
const resolvePaths = [
  path.join(toolDir, 'golden-oracle'),
  path.resolve(process.cwd(), 'tools/golden-oracle'),
]

function resolvePackage(name) {
  try {
    return require.resolve(name, { paths: resolvePaths })
  } catch (error) {
    throw new Error(
      `failed to resolve ${name}. Install the pinned oracle with:\n` +
        `  npm install --prefix tools/golden-oracle\n` +
        `(${error.message})`,
    )
  }
}
async function importPackage(name) {
  return import(pathToFileURL(resolvePackage(name)).href)
}
function arg(name) {
  const i = process.argv.indexOf(name)
  return i >= 0 ? process.argv[i + 1] : undefined
}

const onigModule = await importPackage('vscode-oniguruma')
const onig = onigModule.default ?? onigModule
const onigMain = resolvePackage('vscode-oniguruma')
let wasmPath = path.join(path.dirname(onigMain), 'release', 'onig.wasm')
try { await fs.access(wasmPath) } catch { wasmPath = path.join(path.dirname(onigMain), 'onig.wasm') }
const wasm = await fs.readFile(wasmPath)
await onig.loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength))

const cases = [
  { name: 'dfa-captures', pattern: String.raw`foo(\d+)`, line: 'xxfoo123', engine: 'auto' },
  { name: 'positive-lookahead', pattern: String.raw`foo(?=bar)`, line: 'xxfoobar', engine: 'fallback' },
  { name: 'positive-lookbehind', pattern: String.raw`(?<=foo)bar`, line: 'xxfoobar', engine: 'fallback' },
  { name: 'positive-lookbehind-capture', pattern: String.raw`(?<=(a))b`, line: 'ab', engine: 'fallback' },
  { name: 'lookbehind-capture-backref', pattern: String.raw`(?<=(a))\1`, line: 'aa', engine: 'fallback' },
  { name: 'lookbehind-scoped-flags', pattern: String.raw`(?<=(?i:foo))bar`, line: 'FOObar', engine: 'fallback' },
  { name: 'atomic-ordered-failure', pattern: String.raw`(?>a|ab)c`, line: 'abc', engine: 'fallback', expectMiss: true },
  { name: 'atomic-ordered-match', pattern: String.raw`(?>ab|a)c`, line: 'abc', engine: 'fallback' },
  { name: 'compound-possessive-failure', pattern: String.raw`(a|ab)++c`, line: 'abc', engine: 'fallback', expectMiss: true },
  { name: 'bounded-empty-repeat', pattern: String.raw`(?:){2}a`, line: 'a', engine: 'fallback' },
  { name: 'variable-lookbehind-capture', pattern: String.raw`(?<=(a|aa))b`, line: 'aab', engine: 'fallback' },
  { name: 'bounded-possessive-inner-backtrack', pattern: String.raw`(a|ab){1}+c`, line: 'abc', engine: 'fallback' },
  { name: 'bounded-possessive-zero-width', pattern: String.raw`(a?){2}+a`, line: 'a', engine: 'fallback' },
  { name: 'numbered-backref', pattern: String.raw`(foo)\1`, line: 'xxfoofoo', engine: 'fallback' },
  { name: 'numbered-conditional-matched', pattern: String.raw`(a)?(?(1)b|c)d`, line: 'abd', engine: 'fallback' },
  { name: 'numbered-conditional-unmatched', pattern: String.raw`(a)?(?(1)b|c)d`, line: 'cd', engine: 'fallback' },
  { name: 'named-conditional-matched', pattern: String.raw`(?<x>a)?(?(<x>)b|c)d`, line: 'abd', engine: 'fallback' },
  { name: 'named-conditional-unmatched', pattern: String.raw`(?<x>a)?(?(<x>)b|c)d`, line: 'cd', engine: 'fallback' },
  { name: 'line-anchor-resume-miss', pattern: String.raw`^foo`, line: 'foo', engine: 'auto', from: 1, expectMiss: true },
]

const records = []
for (const c of cases) {
  const scanner = new onig.OnigScanner([c.pattern])
  const onigMatch = c.expectMiss ? null : scanner.findNextMatchSync(c.line, c.from ?? 0)
  const ours = runMark(c)
  const pass = c.expectMiss
    ? ours.match == null && onigMatch == null
    : capturesEqual(ours, onigMatch)
  records.push({ ...c, onig: simplifyOnig(onigMatch), mark: ours, pass })
}
const report = {
  version: 1,
  oracle: 'vscode-oniguruma',
  oraclePackage: 'tools/golden-oracle (vscode-oniguruma@1.7.0)',
  cases: records,
  passed: records.filter(r => r.pass).length,
  failed: records.filter(r => !r.pass).length,
}
await fs.mkdir(path.dirname(out), { recursive: true })
await fs.writeFile(out, JSON.stringify(report, null, 2) + '\n')
console.log(JSON.stringify({ out, passed: report.passed, failed: report.failed }))
if (report.failed) process.exitCode = 1

function runMark(c) {
  const args = ['run', '-q', '-p', 'mark-syntax', '--example', 'regex-parse', '--', '--match', '--engine', c.engine]
  if (c.from != null) args.push('--from', String(c.from))
  args.push(c.pattern, c.line)
  const result = spawnSync('cargo', args, { encoding: 'utf8' })
  if (result.status !== 0) return { error: result.stderr || result.stdout }
  const lines = result.stdout.split(/\r?\n/)
  const matchLine = lines.find(line => line.startsWith('match: '))
  const match = parseSpan(matchLine)
  const captures = lines.filter(line => line.startsWith('capture[')).map(line => parseSpan(line))
  return { match, captures }
}
function parseSpan(line) {
  if (!line || line.includes('<none>') || line.includes('None')) return null
  const m = line.match(/(?:Some\()?([0-9]+)\.\.([0-9]+)/)
  return m ? { start: Number(m[1]), end: Number(m[2]) } : null
}
function simplifyOnig(match) {
  if (!match) return null
  return { index: match.index, captures: match.captureIndices.map(normalizeOnigSpan) }
}
function spansEqual(mark, onig) {
  onig = normalizeOnigSpan(onig)
  if (!mark || !onig) return mark == null && onig == null
  return mark.start === onig.start && mark.end === onig.end
}
function normalizeOnigSpan(span) {
  if (!span || span.start === 0xffffffff || span.end === 0xffffffff) return null
  return { start: span.start, end: span.end }
}
function capturesEqual(mark, onigMatch) {
  if (!onigMatch) return mark.match == null
  const oracle = onigMatch.captureIndices ?? []
  if (!spansEqual(mark.match, oracle[0])) return false
  if (mark.captures.length !== oracle.length) return false
  return oracle.every((capture, index) => spansEqual(mark.captures[index], capture))
}
