import assert from 'node:assert/strict'
import test from 'node:test'

import {
  classificationsFor,
  fixtureKind,
  formatText,
  parseArgs,
  parseManifest,
  selectLanguageCases,
} from './triage-language.mjs'

test('parses and selects only conventional language cases', () => {
  const cases = parseManifest(`
[[case]]
language = "demo" # inline comment
scope = "source.demo"
grammar = "grammar.json"
fixture = "fixtures/demo/basic.demo"
golden = "fixtures/demo/basic.golden.jsonl"

[[case.embedded]]
scope = "source.embedded"
grammar = "embedded.json"

[[case]]
language = "demo"
scope = "source.demo"
grammar = "grammar.json"
fixture = "fixtures/demo/custom.demo"
golden = "fixtures/demo/custom.golden.jsonl"

[[case]]
language = "other"
scope = "source.other"
grammar = "other.json"
fixture = "fixtures/other/stress.other"
golden = "fixtures/other/stress.golden.jsonl"
`)

  assert.equal(cases.length, 3)
  assert.deepEqual(cases[0].embedded, [{ scope: 'source.embedded', grammar: 'embedded.json' }])
  assert.deepEqual(selectLanguageCases(cases, 'demo').map(item => item.kind), ['basic'])
  assert.equal(fixtureKind('somewhere/smoke.Dockerfile'), 'smoke')
  assert.equal(fixtureKind('somewhere/libcxx_vector.cpp'), null)
})

test('parses report and performance options', () => {
  const args = parseArgs([
    'rust', '--json', '--golden', '--kind=stress', '--perf-floor', '3.5',
    '--perf-iterations=5', '--perf-bytes', '120000', '--max-details=4',
  ])
  assert.equal(args.language, 'rust')
  assert.equal(args.oracle, 'golden')
  assert.deepEqual(args.kinds, ['stress'])
  assert.equal(args.perfFloor, 3.5)
  assert.equal(args.perfIterations, 5)
  assert.equal(args.perfBytes, 120000)
  assert.equal(args.maxDetails, 4)
  assert.equal(args.json, true)
})

test('classifies independent blockers and exact parity', () => {
  const exact = classificationsFor({
    comparison: { equal: true },
    counters: { degraded_lines: 0, fallback_budget_kills: 0 },
    stoppedEarlyLines: [],
    performance: { measured: true, mbPerSecond: 2, floorMbPerSecond: 2 },
  })
  assert.deepEqual(exact, ['parity-exact'])

  const blocked = classificationsFor({
    comparison: { equal: false },
    counters: { degraded_lines: 2, fallback_budget_kills: 1 },
    stoppedEarlyLines: [7],
    performance: { measured: true, mbPerSecond: 1.5, floorMbPerSecond: 2 },
  })
  assert.deepEqual(blocked, [
    'oracle-stopped-early',
    'scope-mismatch',
    'degraded',
    'budget-kill',
    'perf-floor',
  ])
})

test('text report includes mismatch lines, counters, and performance', () => {
  const text = formatText({
    ok: false,
    language: 'demo',
    oracle: 'fresh',
    blockers: 1,
    classifications: ['scope-mismatch'],
    cases: [{
      blocker: true,
      kind: 'stress',
      fixture: 'fixtures/demo/stress.demo',
      classifications: ['scope-mismatch'],
      comparison: {
        matchingLines: 2,
        oracleLines: 3,
        equal: false,
        divergentLines: [2],
        missingLines: [],
        extraLines: [],
        reorderedLines: [],
      },
      oracle: { stoppedEarlyLines: [] },
      counters: {
        degradedLines: 0,
        fallbackBudgetKills: 0,
        linesSkipped: 0,
        fallbackStepsTotal: 12,
      },
      performance: {
        mbPerSecond: 3,
        samplesMbPerSecond: [2.9, 3, 3.1],
        floorMbPerSecond: 2,
        measuredBytes: 100000,
      },
    }],
  })
  assert.match(text, /BLOCKED demo/)
  assert.match(text, /divergent: 2/)
  assert.match(text, /degraded=0 budget-kills=0/)
  assert.match(text, /perf: 3\.00 MB\/s/)
})
