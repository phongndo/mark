#!/usr/bin/env node
/**
 * Dev-only TextMate oracle dumper.
 *
 * Runs pinned vscode-textmate + vscode-oniguruma over a source file and writes
 * one JSON object per line (JSONL) with UTF-16 token offsets, full scope stacks,
 * final rule-stack debug text, ruleStackHash, and stoppedEarly.
 *
 * Dependencies resolve only from tools/golden-oracle (see package.json pins).
 * Not used by release builds.
 */
import fs from 'node:fs/promises'
import { createHash } from 'node:crypto'
import path from 'node:path'
import process from 'node:process'
import { createRequire } from 'node:module'
import { fileURLToPath, pathToFileURL } from 'node:url'

function usage() {
  console.log(`usage: golden-dump.mjs --scope <scopeName> --file <source> [options]

Options:
  --assets <directory>         Register every *.json grammar in a directory.
  --grammar <tmLanguage.json>  Register the root grammar directly.
  --language <id>              Language id to write in each record (defaults to --scope).
  --out <jsonl>                Write JSONL to this file instead of stdout.
  --embedded <scope=grammar>   Register an embedded grammar; may be repeated.
  --theme <theme.json>         Also emit resolved TextMate foreground/background/fontStyle.
  --time-limit <ms>            vscode-textmate tokenizeLine time limit (default: 0 = none).
  -h, --help                   Show this help.

Runs vscode-textmate + vscode-oniguruma and writes one JSON object per source line
with UTF-16 token offsets, scope stacks, final rule-stack debug text,
ruleStackHash (sha256 of String(ruleStack)), and stoppedEarly.

Oracle packages resolve from tools/golden-oracle only (dev-only, pinned).`)
}

function parseArgs(argv) {
  const args = { embedded: [], timeLimit: 0 }
  for (let index = 2; index < argv.length; index++) {
    const item = argv[index]
    if (item === '-h' || item === '--help') {
      args.help = true
      continue
    }
    const equals = item.indexOf('=')
    const name = equals > 0 ? item.slice(0, equals) : item
    let value = equals > 0 ? item.slice(equals + 1) : undefined
    if (['--assets', '--grammar', '--scope', '--language', '--file', '--out', '--embedded', '--theme', '--time-limit'].includes(name)) {
      if (value === undefined) {
        value = argv[++index]
      }
      if (value === undefined || value.startsWith('--')) {
        throw new Error(`${name} requires a value`)
      }
      if (name === '--embedded') args.embedded.push(value)
      else if (name === '--time-limit') {
        const parsed = Number(value)
        if (!Number.isFinite(parsed) || parsed < 0) {
          throw new Error(`--time-limit must be a non-negative number, got ${JSON.stringify(value)}`)
        }
        args.timeLimit = parsed
      } else args[name.slice(2)] = value
      continue
    }
    throw new Error(`unknown argument: ${item}`)
  }
  return args
}

let args
try {
  args = parseArgs(process.argv)
} catch (error) {
  console.error(error.message)
  usage()
  process.exit(2)
}
if (args.help) {
  usage()
  process.exit(0)
}

const grammarPath = args.grammar
const scopeName = args.scope
const language = args.language ?? scopeName
const sourcePath = args.file
const outPath = args.out
const timeLimit = args.timeLimit
if ((!grammarPath && !args.assets) || !scopeName || !sourcePath) {
  usage()
  process.exit(2)
}

function parseEmbedded(spec) {
  const equals = spec.indexOf('=')
  if (equals <= 0 || equals === spec.length - 1) {
    throw new Error(`invalid --embedded value ${JSON.stringify(spec)}; expected <scope=grammar>`)
  }
  return { scope: spec.slice(0, equals), grammarPath: spec.slice(equals + 1) }
}

const embedded = []
try {
  for (const spec of args.embedded) embedded.push(parseEmbedded(spec))
} catch (error) {
  console.error(error.message)
  usage()
  process.exit(2)
}

const require = createRequire(import.meta.url)
const toolDir = path.dirname(fileURLToPath(import.meta.url))
// Dev-only oracle package root. Do not resolve from the workspace root or
// unrelated sibling projects — that makes output non-reproducible.
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

const vsctmModule = await importPackage('vscode-textmate')
const vsctm = vsctmModule.default ?? vsctmModule
const onigModule = await importPackage('vscode-oniguruma')
const onig = onigModule.default ?? onigModule
const onigMain = resolvePackage('vscode-oniguruma')
let wasmPath = path.join(path.dirname(onigMain), 'release', 'onig.wasm')
try { await fs.access(wasmPath) } catch { wasmPath = path.join(path.dirname(onigMain), 'onig.wasm') }
const wasm = await fs.readFile(wasmPath)
await onig.loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength))

const grammarSpecs = [...embedded]
if (grammarPath) grammarSpecs.unshift({ scope: scopeName, grammarPath })
if (args.assets) {
  const names = (await fs.readdir(args.assets)).filter(name => name.endsWith('.json')).sort()
  for (const name of names) {
    const grammarPath = path.join(args.assets, name)
    const parsed = JSON.parse(await fs.readFile(grammarPath, 'utf8'))
    if (typeof parsed.scopeName === 'string') {
      grammarSpecs.push({ scope: parsed.scopeName, grammarPath, parsed })
    }
  }
}
const grammars = new Map()
for (const spec of grammarSpecs) {
  if (grammars.has(spec.scope)) throw new Error(`duplicate grammar scope ${spec.scope}`)
  grammars.set(spec.scope, spec.parsed ?? JSON.parse(await fs.readFile(spec.grammarPath, 'utf8')))
}
let rawTheme
if (args.theme) {
  const theme = JSON.parse(await fs.readFile(args.theme, 'utf8'))
  const foreground = theme.colors?.['editor.foreground']
  const background = theme.colors?.['editor.background']
  rawTheme = {
    settings: [
      {
        settings: {
          ...(typeof foreground === 'string' ? { foreground } : {}),
          ...(typeof background === 'string' ? { background } : {}),
        },
      },
      ...(Array.isArray(theme.tokenColors) ? theme.tokenColors : []),
    ],
  }
}

const registry = new vsctm.Registry({
  ...(rawTheme ? { theme: rawTheme } : {}),
  onigLib: Promise.resolve({
    createOnigScanner(patterns) { return new onig.OnigScanner(patterns) },
    createOnigString(source) { return new onig.OnigString(source) },
  }),
  loadGrammar: async (scope) => grammars.get(scope) ?? null,
})

const grammar = await registry.loadGrammar(scopeName)
if (!grammar) throw new Error(`failed to load grammar ${scopeName}`)

const source = await fs.readFile(sourcePath, 'utf8')
const lines = source.split('\n')
let ruleStack = vsctm.INITIAL
const records = []
const colorMap = registry.getColorMap()
for (let lineNumber = 0; lineNumber < lines.length; lineNumber++) {
  const line = lines[lineNumber]
  // timeLimit 0 disables vscode-textmate's wall-clock early stop so long
  // minified fixtures compare against our own budget behavior, not silent
  // oracle truncation.
  const entryRuleStack = ruleStack
  const result = grammar.tokenizeLine(line, entryRuleStack, timeLimit)
  const styled = rawTheme
    ? grammar.tokenizeLine2(line, entryRuleStack, timeLimit)
    : null
  ruleStack = result.ruleStack
  const ruleStackText = String(ruleStack)
  records.push(JSON.stringify({
    language,
    scopeName,
    file: sourcePath,
    lineNumber,
    line,
    tokens: result.tokens.map(token => ({
      startIndex: token.startIndex,
      endIndex: token.endIndex,
      scopes: token.scopes,
      ...(styled ? { style: decodedStyleAt(styled.tokens, token.startIndex, colorMap) } : {}),
    })),
    ruleStack: ruleStackText,
    ruleStackHash: createHash('sha256').update(ruleStackText).digest('hex'),
    stoppedEarly: Boolean(result.stoppedEarly),
  }))
}

const output = `${records.join('\n')}\n`
if (outPath) await fs.writeFile(outPath, output)
else process.stdout.write(output)

function decodedStyleAt(tokens, offset, colorMap) {
  let metadata = 0
  for (let index = 0; index < tokens.length; index += 2) {
    if (tokens[index] > offset) break
    metadata = tokens[index + 1] >>> 0
  }
  const fontStyle = (metadata & 0x00007800) >>> 11
  const foregroundId = (metadata & 0x00ff8000) >>> 15
  const backgroundId = (metadata & 0xff000000) >>> 24
  const modifiers = []
  if (fontStyle & 1) modifiers.push('italic')
  if (fontStyle & 2) modifiers.push('bold')
  if (fontStyle & 4) modifiers.push('underline')
  if (fontStyle & 8) modifiers.push('strikethrough')
  return {
    foreground: colorMap[foregroundId]?.toLowerCase() ?? null,
    background: colorMap[backgroundId]?.toLowerCase() ?? null,
    modifiers,
  }
}
