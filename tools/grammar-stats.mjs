#!/usr/bin/env node
import fs from 'node:fs/promises'
import path from 'node:path'

const root = process.argv[2] ?? 'assets/tm-grammars/languages'
const files = (await fs.readdir(root)).filter(file => file.endsWith('.json')).sort()
const featureKeys = ['lookahead', 'lookbehind', 'backreference', 'anchorA', 'anchorG', 'lineAnchor', 'namedGroup', 'possessiveOrAtomic', 'inlineFlags', 'unicodeOrPosixClass']
const totals = Object.fromEntries(featureKeys.map(key => [key, 0]))
let patternCount = 0
let fallbackPatternCount = 0
let dfaPatternCount = 0
let includeEdges = 0
let injectionCount = 0
const grammars = []

function visit(value, callback) {
  if (Array.isArray(value)) {
    for (const item of value) visit(item, callback)
  } else if (value && typeof value === 'object') {
    callback(value)
    for (const item of Object.values(value)) visit(item, callback)
  }
}

function classify(pattern) {
  return {
    lookahead: /\(\?[=!]/.test(pattern),
    lookbehind: /\(\?<([=!])/.test(pattern),
    backreference: /\\[1-9]/.test(pattern),
    anchorA: /\\A/.test(pattern),
    anchorG: /\\G/.test(pattern),
    lineAnchor: /(^|[^\\])\^/.test(pattern),
    namedGroup: /\(\?(P<|<[^=!])/.test(pattern),
    possessiveOrAtomic: /\(\?>|[+*?}]\+/.test(pattern),
    inlineFlags: /\(\?[imsx-]/.test(pattern),
    unicodeOrPosixClass: /\\[pP]\{|\[:[a-z]+:\]/i.test(pattern),
  }
}

for (const file of files) {
  const full = path.join(root, file)
  const grammar = JSON.parse(await fs.readFile(full, 'utf8'))
  const local = { language: file.replace(/\.tmLanguage\.json$/, ''), scopeName: grammar.scopeName, patterns: 0, dfaPatterns: 0, fallbackPatterns: 0, includes: 0, injections: grammar.injections ? Object.keys(grammar.injections).length : 0, features: Object.fromEntries(featureKeys.map(key => [key, 0])) }
  injectionCount += local.injections
  visit(grammar, node => {
    for (const key of ['match', 'begin', 'end', 'while']) {
      if (typeof node[key] === 'string') {
        patternCount += 1
        local.patterns += 1
        const features = classify(node[key])
        const fallback = features.lookahead || features.lookbehind || features.backreference || features.anchorG || features.namedGroup || features.possessiveOrAtomic
        if (fallback) { fallbackPatternCount += 1; local.fallbackPatterns += 1 }
        else { dfaPatternCount += 1; local.dfaPatterns += 1 }
        for (const feature of featureKeys) {
          if (features[feature]) {
            totals[feature] += 1
            local.features[feature] += 1
          }
        }
      }
    }
    if (typeof node.include === 'string') {
      includeEdges += 1
      local.includes += 1
    }
  })
  grammars.push(local)
}

console.log(JSON.stringify({ grammarCount: grammars.length, patternCount, dfaPatternCount, fallbackPatternCount, fallbackPatternPercent: patternCount ? fallbackPatternCount / patternCount : 0, includeEdges, injectionCount, features: totals, grammars }, null, 2))
