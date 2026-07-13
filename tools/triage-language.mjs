#!/usr/bin/env node
/**
 * Run the complete TextMate triage loop for one language in cases.toml.
 *
 * The checked-in manifest is the source of truth for grammar, fixture, and
 * embedded-grammar paths. Fresh vscode-textmate output is written only to a
 * temporary directory; committed goldens are read-only unless --golden is
 * explicitly selected.
 */
import fs from 'node:fs/promises'
import fsSync from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import process from 'node:process'
import { spawnSync } from 'node:child_process'
import { fileURLToPath, pathToFileURL } from 'node:url'

const TOOL_DIR = path.dirname(fileURLToPath(import.meta.url))
const ROOT = path.dirname(TOOL_DIR)
const DEFAULT_MANIFEST = 'crates/mark-syntax/tests/fixtures/textmate/cases.toml'
const FIXTURE_KINDS = new Set(['basic', 'stress', 'smoke'])
const MAX_BUFFER = 64 * 1024 * 1024

function usage(stream = process.stdout) {
  stream.write(`usage: node tools/triage-language.mjs <language> [options]\n
Locate all committed basic/stress/smoke cases for a language, generate a fresh
oracle in a temporary directory, run native exact parity and counters, and
check repeated-stress process-cold throughput. Blockers produce exit status 1.

Options:
  --json                    Emit one JSON report instead of text.
  --golden                  Compare with checked-in goldens (no Node oracle run).
  --oracle <fresh|golden>   Select oracle source (default: fresh).
  --kind <kind>             Only basic, stress, or smoke; may be repeated.
  --perf-floor <MB/s>       Process-cold stress floor (default: 2).
  --perf-iterations <n>     Fresh-tokenizer timing samples (default: 3).
  --perf-bytes <n>          Minimum repeated stress size (default: 100000).
  --no-perf                 Skip the stress performance measurement.
  --manifest <path>         Alternate cases.toml (default: ${DEFAULT_MANIFEST}).
  --max-details <n>         Scope mismatch line numbers shown in text (default: 10).
  --keep-temp               Keep and report oracle/native temporary files.
  -h, --help                Show this help.
`)
}

export function parseArgs(argv) {
  const args = {
    oracle: 'fresh',
    manifest: DEFAULT_MANIFEST,
    kinds: [],
    perfFloor: 2,
    perfIterations: 3,
    perfBytes: 100_000,
    perf: true,
    json: false,
    keepTemp: false,
    maxDetails: 10,
  }
  const positional = []
  for (let index = 0; index < argv.length; index++) {
    const item = argv[index]
    if (item === '-h' || item === '--help') {
      args.help = true
      continue
    }
    if (item === '--json') {
      args.json = true
      continue
    }
    if (item === '--golden') {
      args.oracle = 'golden'
      continue
    }
    if (item === '--no-perf') {
      args.perf = false
      continue
    }
    if (item === '--keep-temp') {
      args.keepTemp = true
      continue
    }
    const equals = item.indexOf('=')
    const name = equals > 0 ? item.slice(0, equals) : item
    let value = equals > 0 ? item.slice(equals + 1) : undefined
    if (['--oracle', '--manifest', '--kind', '--perf-floor', '--perf-iterations', '--perf-bytes', '--max-details'].includes(name)) {
      if (value === undefined) value = argv[++index]
      if (value === undefined || value.startsWith('--')) throw new Error(`${name} requires a value`)
      if (name === '--oracle') args.oracle = value
      else if (name === '--manifest') args.manifest = value
      else if (name === '--kind') args.kinds.push(value)
      else if (name === '--perf-floor') args.perfFloor = finiteNumber(value, name, 0)
      else if (name === '--perf-iterations') args.perfIterations = integer(value, name, 1)
      else if (name === '--perf-bytes') args.perfBytes = integer(value, name, 1)
      else args.maxDetails = integer(value, name, 1)
      continue
    }
    if (item.startsWith('-')) throw new Error(`unknown argument: ${item}`)
    positional.push(item)
  }
  if (!['fresh', 'golden'].includes(args.oracle)) {
    throw new Error(`--oracle must be fresh or golden, got ${JSON.stringify(args.oracle)}`)
  }
  if (args.kinds.some(kind => !FIXTURE_KINDS.has(kind))) {
    throw new Error('--kind must be basic, stress, or smoke')
  }
  if (positional.length > 1) throw new Error(`unexpected arguments: ${positional.slice(1).join(' ')}`)
  args.language = positional[0]
  return args
}

function finiteNumber(value, name, minimum) {
  const parsed = Number(value)
  if (!Number.isFinite(parsed) || parsed < minimum) {
    throw new Error(`${name} must be a number >= ${minimum}`)
  }
  return parsed
}

function integer(value, name, minimum) {
  const parsed = Number(value)
  if (!Number.isSafeInteger(parsed) || parsed < minimum) {
    throw new Error(`${name} must be an integer >= ${minimum}`)
  }
  return parsed
}

/** Parse the deliberately small cases.toml schema without adding a dependency. */
export function parseManifest(text, filename = '<manifest>') {
  const cases = []
  let currentCase = null
  let target = null
  let kind = null
  for (const [index, raw] of text.split(/\r?\n/).entries()) {
    const lineNumber = index + 1
    const line = stripComment(raw).trim()
    if (!line) continue
    const table = line.match(/^\[\[\s*([A-Za-z0-9_.-]+)\s*\]\]$/)
    if (table) {
      if (table[1] === 'case') {
        currentCase = { embedded: [], __line: lineNumber }
        cases.push(currentCase)
        target = currentCase
        kind = 'case'
      } else if (table[1] === 'case.embedded') {
        if (!currentCase) throw manifestError(filename, lineNumber, '[[case.embedded]] must follow [[case]]')
        target = { __line: lineNumber }
        currentCase.embedded.push(target)
        kind = 'embedded'
      } else {
        throw manifestError(filename, lineNumber, `unsupported table [[${table[1]}]]`)
      }
      continue
    }
    const assignment = line.match(/^([A-Za-z0-9_-]+)\s*=\s*(.+)$/)
    if (!assignment || !target) throw manifestError(filename, lineNumber, 'expected a table or key = "value"')
    const allowed = kind === 'case'
      ? new Set(['language', 'scope', 'grammar', 'fixture', 'golden'])
      : new Set(['scope', 'grammar'])
    const key = assignment[1]
    if (!allowed.has(key)) throw manifestError(filename, lineNumber, `unsupported ${kind} key ${key}`)
    if (Object.hasOwn(target, key)) throw manifestError(filename, lineNumber, `duplicate ${kind} key ${key}`)
    target[key] = parseTomlString(assignment[2].trim(), filename, lineNumber)
  }
  for (const testCase of cases) {
    for (const key of ['language', 'scope', 'grammar', 'fixture', 'golden']) {
      if (testCase[key] === undefined) throw manifestError(filename, testCase.__line, `missing case key ${key}`)
    }
    for (const embedded of testCase.embedded) {
      for (const key of ['scope', 'grammar']) {
        if (embedded[key] === undefined) throw manifestError(filename, embedded.__line, `missing embedded key ${key}`)
      }
      delete embedded.__line
    }
    delete testCase.__line
  }
  return cases
}

function stripComment(line) {
  let quote = null
  let escaped = false
  let result = ''
  for (const character of line) {
    if (quote) {
      result += character
      if (quote === '"' && escaped) escaped = false
      else if (quote === '"' && character === '\\') escaped = true
      else if (character === quote) quote = null
    } else if (character === '"' || character === "'") {
      quote = character
      result += character
    } else if (character === '#') {
      break
    } else {
      result += character
    }
  }
  return result
}

function parseTomlString(value, filename, lineNumber) {
  if (value.startsWith('"') && value.endsWith('"')) {
    try {
      const parsed = JSON.parse(value)
      if (typeof parsed === 'string') return parsed
    } catch {}
  }
  if (value.startsWith("'") && value.endsWith("'")) return value.slice(1, -1)
  throw manifestError(filename, lineNumber, 'only single-line string values are supported')
}

function manifestError(filename, lineNumber, message) {
  return new Error(`${filename}:${lineNumber}: ${message}`)
}

export function fixtureKind(fixture) {
  return path.basename(fixture).match(/^(basic|stress|smoke)(?:\.|$)/)?.[1] ?? null
}

export function selectLanguageCases(cases, language, kinds = []) {
  const selectedKinds = new Set(kinds.length ? kinds : FIXTURE_KINDS)
  return cases
    .map((testCase, manifestIndex) => ({ ...testCase, manifestIndex, kind: fixtureKind(testCase.fixture) }))
    .filter(testCase => testCase.language === language && selectedKinds.has(testCase.kind))
}

export function classificationsFor({ comparison, counters, stoppedEarlyLines, performance }) {
  const classifications = []
  if (stoppedEarlyLines.length) classifications.push('oracle-stopped-early')
  if (!comparison.equal) classifications.push('scope-mismatch')
  if (Number(counters.degraded_lines ?? 0) > 0) classifications.push('degraded')
  if (Number(counters.fallback_budget_kills ?? 0) > 0) classifications.push('budget-kill')
  if (performance?.measured && performance.mbPerSecond < performance.floorMbPerSecond) {
    classifications.push('perf-floor')
  }
  return classifications.length ? classifications : ['parity-exact']
}

function primaryClassification(classifications) {
  for (const name of ['oracle-stopped-early', 'budget-kill', 'degraded', 'scope-mismatch', 'perf-floor']) {
    if (classifications.includes(name)) return name
  }
  return 'parity-exact'
}

function resolveFromRoot(filePath) {
  return path.isAbsolute(filePath) ? filePath : path.resolve(ROOT, filePath)
}

function relative(filePath) {
  const value = path.relative(ROOT, filePath)
  return value && !value.startsWith('..') ? value : filePath
}

function command(commandName, args, options = {}) {
  const result = spawnSync(commandName, args, {
    cwd: ROOT,
    encoding: 'utf8',
    maxBuffer: MAX_BUFFER,
    ...options,
  })
  if (result.error) throw new Error(`failed to run ${commandName}: ${result.error.message}`)
  return result
}

function requireSuccess(result, description) {
  if (result.status === 0) return
  const detail = [result.stderr, result.stdout].filter(Boolean).join('\n').trim()
  throw new Error(`${description} failed (exit ${result.status})${detail ? `:\n${detail}` : ''}`)
}

async function readJsonLines(filePath) {
  const text = await fs.readFile(filePath, 'utf8')
  const records = []
  for (const [index, line] of text.split(/\r?\n/).entries()) {
    if (!line.trim()) continue
    try {
      records.push(JSON.parse(line))
    } catch (error) {
      throw new Error(`${filePath}:${index + 1}: invalid JSON: ${error.message}`)
    }
  }
  return records
}

function summarizeComparison(comparison) {
  return {
    oracleLines: comparison.oracleLines,
    nativeLines: comparison.nativeLines,
    matchingLines: comparison.matchingLines,
    divergentLines: comparison.divergentLines,
    missingLines: comparison.missingLines,
    extraLines: comparison.extraLines,
    reorderedLines: comparison.reorderedLines,
    equal: comparison.equal,
  }
}

function summarizeCounters(counters) {
  return {
    linesTokenized: counters.lines_tokenized ?? 0,
    linesSkipped: counters.lines_skipped ?? 0,
    degradedLines: counters.degraded_lines ?? 0,
    fallbackBudgetKills: counters.fallback_budget_kills ?? 0,
    fallbackStepsTotal: counters.fallback_steps_total ?? 0,
    fallbackStepsMax: counters.fallback_steps_max ?? 0,
    regexDfaAttempts: counters.regex_dfa_attempts ?? 0,
    regexFallbackAttempts: counters.regex_fallback_attempts ?? 0,
  }
}

async function dumpFreshOracle(testCase, outPath) {
  const args = [
    path.join(TOOL_DIR, 'golden-dump.mjs'),
    '--grammar', resolveFromRoot(testCase.grammar),
    '--scope', testCase.scope,
    '--language', testCase.language,
    '--file', resolveFromRoot(testCase.fixture),
    '--out', outPath,
  ]
  for (const embedded of testCase.embedded) {
    args.push('--embedded', `${embedded.scope}=${resolveFromRoot(embedded.grammar)}`)
  }
  const result = command(process.execPath, args)
  requireSuccess(result, `oracle dump for ${testCase.fixture}`)
}

async function runNative(testCase, nativePath, countersPath, binary) {
  const args = [
    '--grammar', resolveFromRoot(testCase.grammar),
    '--counters', countersPath,
  ]
  for (const embedded of testCase.embedded) args.push('--embedded', resolveFromRoot(embedded.grammar))
  args.push(resolveFromRoot(testCase.fixture))
  const descriptor = fsSync.openSync(nativePath, 'w')
  let result
  try {
    result = command(binary, args, { stdio: ['ignore', descriptor, 'pipe'] })
  } finally {
    fsSync.closeSync(descriptor)
  }
  requireSuccess(result, `native tokenize for ${testCase.fixture}`)
  return JSON.parse(await fs.readFile(countersPath, 'utf8'))
}

async function compareScopes(oraclePath, nativePath) {
  const result = command('python3', [
    path.join(TOOL_DIR, 'compare-textmate-scopes.py'),
    oraclePath,
    nativePath,
    '--json',
  ])
  if (![0, 1].includes(result.status)) requireSuccess(result, 'scope comparison')
  try {
    return JSON.parse(result.stdout)
  } catch (error) {
    throw new Error(`scope comparison returned invalid JSON: ${error.message}`)
  }
}

async function makeRepeatedStress(fixture, outPath, minimumBytes) {
  let seed = await fs.readFile(resolveFromRoot(fixture))
  if (!seed.length) throw new Error(`cannot performance-test empty stress fixture ${fixture}`)
  if (seed[seed.length - 1] !== 0x0a) seed = Buffer.concat([seed, Buffer.from('\n')])
  const repeats = Math.max(1, Math.ceil(minimumBytes / seed.length))
  const output = Buffer.allocUnsafe(seed.length * repeats)
  for (let index = 0; index < repeats; index++) seed.copy(output, index * seed.length)
  await fs.writeFile(outPath, output)
  return { bytes: output.length, repeats }
}

function median(values) {
  const sorted = [...values].sort((left, right) => left - right)
  const middle = Math.floor(sorted.length / 2)
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2
}

async function measurePerformance(testCase, outPath, binary, args) {
  const repeated = await makeRepeatedStress(testCase.fixture, outPath, args.perfBytes)
  const nativeArgs = [
    '--mode', 'process-cold',
    '--grammar', resolveFromRoot(testCase.grammar),
  ]
  for (const embedded of testCase.embedded) nativeArgs.push('--embedded', resolveFromRoot(embedded.grammar))
  nativeArgs.push(outPath, '1')
  const samples = []
  for (let index = 0; index < args.perfIterations; index++) {
    // Keep process-cold samples in separate processes. Diagnostic counters and
    // scope serialization are intentionally performed in other passes.
    const result = command(binary, nativeArgs)
    requireSuccess(result, `process-cold performance sample ${index + 1} for ${testCase.fixture}`)
    const output = `${result.stdout ?? ''}\n${result.stderr ?? ''}`
    const matches = [...output.matchAll(/iter\s+\d+\s+mode=process-cold:.*?([0-9]+(?:\.[0-9]+)?)\s+MB\/s/g)]
    if (matches.length !== 1) {
      throw new Error(`could not parse process-cold performance sample ${index + 1} for ${testCase.fixture}:\n${output.trim()}`)
    }
    samples.push(Number(matches[0][1]))
  }
  return {
    measured: true,
    mode: 'process-cold',
    fixtureRepeated: repeated.repeats,
    measuredBytes: repeated.bytes,
    iterations: samples.length,
    samplesMbPerSecond: samples,
    mbPerSecond: median(samples),
    floorMbPerSecond: args.perfFloor,
    passes: median(samples) >= args.perfFloor,
  }
}

function buildNativeExamples() {
  const result = command('cargo', [
    'build', '--release', '--locked', '-p', 'mark-syntax',
    '--example', 'tokenize', '--example', 'profile-cold',
    '--message-format=json-render-diagnostics',
  ])
  requireSuccess(result, 'native triage example build')
  const executables = new Map()
  for (const line of result.stdout.split(/\r?\n/)) {
    if (!line.trim()) continue
    let message
    try {
      message = JSON.parse(line)
    } catch {
      continue
    }
    if (message.reason === 'compiler-artifact' && message.executable
      && message.target?.kind?.includes('example')) {
      executables.set(message.target.name, message.executable)
    }
  }
  for (const name of ['tokenize', 'profile-cold']) {
    if (!executables.has(name)) {
      throw new Error(`Cargo did not report an executable for example ${name}`)
    }
  }
  return executables
}

export async function runTriage(args) {
  const manifestPath = path.isAbsolute(args.manifest) ? args.manifest : path.resolve(ROOT, args.manifest)
  const manifestText = await fs.readFile(manifestPath, 'utf8')
  const allCases = parseManifest(manifestText, relative(manifestPath))
  const selected = selectLanguageCases(allCases, args.language, args.kinds)
  if (!selected.length) {
    const languages = [...new Set(allCases.filter(item => fixtureKind(item.fixture)).map(item => item.language))].sort()
    throw new Error(`no committed basic/stress/smoke cases matched language ${JSON.stringify(args.language)}\navailable: ${languages.join(', ')}`)
  }
  for (const testCase of selected) {
    const requiredFiles = [testCase.grammar, testCase.fixture, ...testCase.embedded.map(item => item.grammar)]
    if (args.oracle === 'golden') requiredFiles.push(testCase.golden)
    for (const file of requiredFiles) {
      await fs.access(resolveFromRoot(file))
    }
  }

  const executables = buildNativeExamples()
  const tokenizeBinary = executables.get('tokenize')
  const profileBinary = executables.get('profile-cold')
  const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), `mark-triage-${args.language.replace(/[^A-Za-z0-9_.-]/g, '_')}-`))
  const reports = []
  try {
    for (const [selectedIndex, testCase] of selected.entries()) {
      const stem = `${String(selectedIndex).padStart(2, '0')}-${testCase.kind}`
      const generatedOraclePath = path.join(tempRoot, `${stem}.oracle.jsonl`)
      const oraclePath = args.oracle === 'golden' ? resolveFromRoot(testCase.golden) : generatedOraclePath
      const nativePath = path.join(tempRoot, `${stem}.native.jsonl`)
      const countersPath = path.join(tempRoot, `${stem}.counters.json`)
      if (args.oracle === 'fresh') await dumpFreshOracle(testCase, oraclePath)
      const oracleRecords = await readJsonLines(oraclePath)
      const stoppedEarlyLines = oracleRecords
        .filter(record => record.stoppedEarly !== false)
        .map(record => record.lineNumber)
      const rawCounters = await runNative(testCase, nativePath, countersPath, tokenizeBinary)
      const comparison = await compareScopes(oraclePath, nativePath)
      let performance = null
      if (args.perf && testCase.kind === 'stress') {
        performance = await measurePerformance(
          testCase,
          path.join(tempRoot, `${stem}.perf${path.extname(testCase.fixture)}`),
          profileBinary,
          args,
        )
      }
      const classifications = classificationsFor({ comparison, counters: rawCounters, stoppedEarlyLines, performance })
      reports.push({
        kind: testCase.kind,
        fixture: testCase.fixture,
        grammar: testCase.grammar,
        scope: testCase.scope,
        embeddedScopes: testCase.embedded.map(item => item.scope),
        oracle: {
          source: args.oracle,
          stoppedEarlyLines,
          path: args.keepTemp || args.oracle === 'golden' ? relative(oraclePath) : undefined,
        },
        comparison: summarizeComparison(comparison),
        counters: summarizeCounters(rawCounters),
        performance,
        classification: primaryClassification(classifications),
        classifications,
        blocker: classifications[0] !== 'parity-exact',
        artifacts: args.keepTemp ? {
          native: nativePath,
          counters: countersPath,
        } : undefined,
      })
    }

    const classifications = [...new Set(reports.flatMap(item => item.classifications))]
    const blockers = reports.filter(item => item.blocker).length
    return {
      version: 1,
      language: args.language,
      manifest: relative(manifestPath),
      oracle: args.oracle,
      performanceFloorMbPerSecond: args.perf ? args.perfFloor : null,
      cases: reports,
      classifications: blockers ? classifications.filter(item => item !== 'parity-exact') : ['parity-exact'],
      blockers,
      ok: blockers === 0,
      tempDirectory: args.keepTemp ? tempRoot : undefined,
    }
  } catch (error) {
    if (args.keepTemp) error.tempDirectory = tempRoot
    throw error
  } finally {
    if (!args.keepTemp) await fs.rm(tempRoot, { recursive: true, force: true })
  }
}

function list(values, limit) {
  if (!values?.length) return 'none'
  const shown = values.slice(0, limit).map(value => typeof value === 'object' ? value.lineNumber : value)
  return `${shown.join(', ')}${values.length > limit ? `, … (+${values.length - limit})` : ''}`
}

export function formatText(report, maxDetails = 10) {
  const lines = [
    `${report.ok ? 'PASS' : 'BLOCKED'} ${report.language}: ${report.cases.length} case(s), oracle=${report.oracle}`,
  ]
  for (const testCase of report.cases) {
    lines.push(`${testCase.blocker ? '✗' : '✓'} ${testCase.kind} ${testCase.fixture}`)
    lines.push(`  classification: ${testCase.classifications.join(', ')}`)
    const comparison = testCase.comparison
    lines.push(`  scopes: ${comparison.matchingLines}/${comparison.oracleLines} lines exact`)
    if (!comparison.equal) {
      lines.push(`  divergent: ${list(comparison.divergentLines, maxDetails)}`)
      lines.push(`  missing: ${list(comparison.missingLines, maxDetails)}; extra: ${list(comparison.extraLines, maxDetails)}; reordered: ${list(comparison.reorderedLines, maxDetails)}`)
    }
    if (testCase.oracle.stoppedEarlyLines.length) {
      lines.push(`  oracle stopped/invalid: ${list(testCase.oracle.stoppedEarlyLines, maxDetails)}`)
    }
    const counters = testCase.counters
    lines.push(`  counters: degraded=${counters.degradedLines} budget-kills=${counters.fallbackBudgetKills} skipped=${counters.linesSkipped} fallback-steps=${counters.fallbackStepsTotal}`)
    if (testCase.performance) {
      lines.push(`  perf: ${testCase.performance.mbPerSecond.toFixed(2)} MB/s median (${testCase.performance.samplesMbPerSecond.join(', ')}), floor=${testCase.performance.floorMbPerSecond.toFixed(2)} MB/s, bytes=${testCase.performance.measuredBytes}`)
    } else if (testCase.kind === 'stress') {
      lines.push('  perf: skipped')
    }
  }
  lines.push(`Summary: ${report.ok ? 'parity-exact; no blockers' : `${report.blockers} blocker case(s): ${report.classifications.join(', ')}`}`)
  if (report.tempDirectory) lines.push(`Artifacts: ${report.tempDirectory}`)
  return lines.join('\n')
}

async function main() {
  let args
  try {
    args = parseArgs(process.argv.slice(2))
  } catch (error) {
    if (process.argv.slice(2).includes('--json')) {
      console.log(JSON.stringify({ version: 1, ok: false, blockers: 1, errors: [error.message] }, null, 2))
    } else {
      console.error(`error: ${error.message}`)
      usage(process.stderr)
    }
    return 2
  }
  if (args.help) {
    usage()
    return 0
  }
  if (!args.language) {
    if (args.json) {
      console.log(JSON.stringify({ version: 1, ok: false, blockers: 1, errors: ['missing language'] }, null, 2))
    } else {
      console.error('error: missing language')
      usage(process.stderr)
    }
    return 2
  }
  try {
    const report = await runTriage(args)
    console.log(args.json ? JSON.stringify(report, null, 2) : formatText(report, args.maxDetails))
    return report.ok ? 0 : 1
  } catch (error) {
    if (args.json) {
      console.log(JSON.stringify({
        version: 1,
        language: args.language,
        ok: false,
        blockers: 1,
        errors: [error.message],
        tempDirectory: error.tempDirectory,
      }, null, 2))
    } else {
      console.error(`error: ${error.message}`)
      if (error.tempDirectory) console.error(`artifacts: ${error.tempDirectory}`)
    }
    return 2
  }
}

const invokedPath = process.argv[1] ? pathToFileURL(path.resolve(process.argv[1])).href : null
if (invokedPath === import.meta.url) process.exitCode = await main()
