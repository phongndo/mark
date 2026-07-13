import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


SPEC = importlib.util.spec_from_file_location(
    "check_language_docs", Path(__file__).with_name("check-language-docs.py")
)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class LanguageDocFreshnessTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.counts = MODULE.Counts(
            supported=254,
            validated=12,
            oracle=42,
            corpus=27,
            manifest_cases=59,
            validated_ids=("asm", "zig"),
        )

    def tearDown(self):
        self.temporary.cleanup()

    def policy(self, **decision):
        policy = {
            "schemaVersion": 1,
            "targetPublicLanguages": 254,
            "targetMinimumGoldenCases": 508,
            "measurementGateCases": 124,
            "warmupRuns": 1,
            "timedRuns": 5,
            "interactiveP95Seconds": 60,
            "shardTargetP95Seconds": 45,
            "maximumShards": 8,
            "shardStrategy": "sha256-language-id-modulo",
            "referenceRunner": "GitHub Actions quality job (ubuntu-latest)",
            "percentileMethod": "nearest-rank",
            "timingCommand": "cargo test -p mark-syntax --test textmate_golden --locked",
            "decision": decision or {"state": "pending"},
        }
        path = self.root / MODULE.SCALE_POLICY
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(policy))
        return policy

    def write_doc_skeletons(self, top="no public language count here\n"):
        for path in MODULE.MANAGED_DOCS:
            full = self.root / path
            full.parent.mkdir(parents=True, exist_ok=True)
            scale = ""
            if path == Path("docs/textmate-engine.md"):
                scale = f"\n{MODULE.SCALE_START}\nold\n{MODULE.SCALE_END}\n"
            full.write_text(f"{MODULE.COUNT_START}\nold\n{MODULE.COUNT_END}{scale}")
        (self.root / MODULE.TOP_README).write_text(top)

    def write_count_inputs(self, oracle=2):
        inputs = {
            MODULE.COVERAGE: """public_language_count = 2
kept = [
  "alpha",
  "beta",
]
""",
            MODULE.CASES: """[[case]]
language = "alpha"
grammar = "assets/alpha.tmLanguage.json"

[[case]]
language = "fixture-alias"
grammar = "assets/beta.tmLanguage.json"
""",
            MODULE.CORPORA: """[[corpus]]
id = "catalog-repeated"
languages = 1
""",
            MODULE.STATUS: f"""Mark bundles **2 supported public language IDs**. **1 are validated** by the contract; **1 more are supported**.

The oracle manifest currently covers **{oracle}** public IDs. `catalog-repeated` contains the **1** IDs with stress fixtures.

| Language ID | Tier | Status | Oracle fixtures |
| `alpha` | A | validated | `basic`, `stress` |
| `beta` | B | supported | `smoke` |
""",
            MODULE.VALIDATION_POLICY: json.dumps(
                {
                    "expectedCounts": {
                        "publicLanguages": 2,
                        "validatedLanguages": 1,
                        "oracleLanguages": 2,
                        "stressCorpusLanguages": 1,
                    }
                }
            ),
        }
        for path, text in inputs.items():
            full = self.root / path
            full.parent.mkdir(parents=True, exist_ok=True)
            full.write_text(text)

    def test_snippets_are_derived_from_one_count_snapshot(self):
        snippet = MODULE.count_snippet(Path("docs/configuration.md"), self.counts)
        self.assertIn("254 public language IDs", snippet)
        self.assertIn("12 are validated", snippet)
        self.assertIn("242 more are supported", snippet)

        fixture = MODULE.count_snippet(MODULE.FIXTURE_README, self.counts)
        self.assertIn("59 cases", fixture)
        self.assertIn("42 public language IDs", fixture)
        self.assertIn("27 IDs", fixture)

    def test_manifest_status_and_corpus_counts_are_cross_checked(self):
        self.write_count_inputs()
        counts = MODULE.collect_counts(self.root)
        self.assertEqual((counts.supported, counts.validated), (2, 1))
        self.assertEqual((counts.oracle, counts.corpus, counts.manifest_cases), (2, 1, 2))

        self.write_count_inputs(oracle=1)
        counts = MODULE.collect_counts(self.root)
        self.assertEqual(counts.oracle, 2, "generated manifest is the oracle-count source")

    def test_locked_validation_counts_are_cross_checked(self):
        self.write_count_inputs()
        policy_path = self.root / MODULE.VALIDATION_POLICY
        policy = json.loads(policy_path.read_text())
        policy["expectedCounts"]["validatedLanguages"] = 2
        policy_path.write_text(json.dumps(policy))
        with self.assertRaisesRegex(ValueError, "validatedLanguages=2, found 1"):
            MODULE.collect_counts(self.root)

    def test_duplicate_or_missing_markers_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "exactly one"):
            MODULE.replace_snippet("no markers", "start", "end", "new", "doc.md")
        with self.assertRaisesRegex(ValueError, "exactly one"):
            MODULE.replace_snippet("start end start end", "start", "end", "new", "doc.md")

    def test_top_readme_count_claim_must_be_managed(self):
        self.write_doc_skeletons("Syntax highlighting for 254 languages.\n")
        policy = self.policy()
        with self.assertRaisesRegex(ValueError, "unmanaged language-count claim"):
            MODULE.render_docs(self.root, self.counts, policy)

    def test_top_readme_generated_claim_is_refreshed_when_present(self):
        top = f"{MODULE.COUNT_START}\nold\n{MODULE.COUNT_END}\n"
        self.write_doc_skeletons(top)
        rendered = MODULE.render_docs(self.root, self.counts, self.policy())
        self.assertIn("254 supported public language IDs", rendered[MODULE.TOP_README])
        self.assertIn("42 oracle-covered", rendered[MODULE.TOP_README])

    def test_managed_doc_rejects_a_second_hand_written_count(self):
        self.write_doc_skeletons()
        path = self.root / Path("docs/configuration.md")
        path.write_text(path.read_text() + "\nPreviously 99 were validated.\n")
        with self.assertRaisesRegex(ValueError, "unmanaged language-count claim"):
            MODULE.render_docs(self.root, self.counts, self.policy())

    def test_pending_timing_decision_is_allowed_before_scale_gate(self):
        policy = self.policy()
        self.assertEqual(MODULE.load_scale_policy(self.root, self.counts), policy)

    def test_pending_timing_decision_is_rejected_at_scale_gate(self):
        self.policy()
        at_gate = MODULE.Counts(254, 12, 42, 27, 124, ("asm", "zig"))
        with self.assertRaisesRegex(ValueError, "cannot remain pending"):
            MODULE.load_scale_policy(self.root, at_gate)

    def test_slow_measurement_requires_compliant_shards(self):
        measured_counts = MODULE.Counts(254, 12, 42, 27, 124, ("asm", "zig"))
        self.policy(
            state="measured",
            manifestCases=124,
            fullSuiteP95Seconds=61,
            interactiveThresholdSeconds=60,
            shardCount=1,
            measuredAt="2026-07-12",
            runner="GitHub Actions quality job (ubuntu-latest)",
            evidence="local timing",
        )
        with self.assertRaisesRegex(ValueError, "must be sharded"):
            MODULE.load_scale_policy(self.root, measured_counts)

        self.policy(
            state="measured",
            manifestCases=124,
            fullSuiteP95Seconds=61,
            interactiveThresholdSeconds=60,
            shardCount=2,
            maximumShardP95SecondsByShardCount={"1": 61, "2": 40},
            measuredAt="2026-07-12",
            runner="GitHub Actions quality job (ubuntu-latest)",
            evidence="local timing",
        )
        MODULE.load_scale_policy(self.root, measured_counts)

    def test_per_shard_evidence_must_cover_indexes_and_meet_target(self):
        measured_counts = MODULE.Counts(254, 12, 42, 27, 124, ("asm", "zig"))
        base = dict(
            state="measured",
            manifestCases=124,
            fullSuiteP95Seconds=61,
            interactiveThresholdSeconds=60,
            shardCount=2,
            measuredAt="2026-07-12",
            runner="GitHub Actions quality job (ubuntu-latest)",
            evidence="local timing",
        )

        self.policy(**base, shardP95SecondsByIndex={"0": 31, "1": 33})
        MODULE.load_scale_policy(self.root, measured_counts)

        self.policy(**base, shardP95SecondsByIndex={"0": 31})
        with self.assertRaisesRegex(ValueError, "selected shard indexes"):
            MODULE.load_scale_policy(self.root, measured_counts)

        self.policy(**base, shardP95SecondsByIndex={"0": 31, "1": 46})
        with self.assertRaisesRegex(ValueError, "exceeds the shard p95 target"):
            MODULE.load_scale_policy(self.root, measured_counts)

    def test_measured_threshold_must_match_policy(self):
        measured_counts = MODULE.Counts(254, 12, 42, 27, 124, ("asm", "zig"))
        self.policy(
            state="measured",
            manifestCases=124,
            fullSuiteP95Seconds=61,
            interactiveThresholdSeconds=59,
            shardCount=4,
            measuredAt="2026-07-12",
            runner="GitHub Actions quality job (ubuntu-latest)",
            evidence="local timing",
        )
        with self.assertRaisesRegex(ValueError, "threshold must match"):
            MODULE.load_scale_policy(self.root, measured_counts)

    def test_measured_case_count_must_match_current_manifest(self):
        self.policy(
            state="measured",
            manifestCases=124,
            fullSuiteP95Seconds=61,
            interactiveThresholdSeconds=60,
            shardCount=4,
            measuredAt="2026-07-12",
            runner="GitHub Actions quality job (ubuntu-latest)",
            evidence="local timing",
        )
        with self.assertRaisesRegex(ValueError, "stale"):
            MODULE.load_scale_policy(self.root, self.counts)


if __name__ == "__main__":
    unittest.main()
