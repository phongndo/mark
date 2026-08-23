import importlib.util
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


def load_generator():
    path = ROOT / "scripts/ci/generate-homebrew-formula.py"
    spec = importlib.util.spec_from_file_location("generate_homebrew_formula", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


generator = load_generator()


class HomebrewFormulaTests(unittest.TestCase):
    def setUp(self):
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.checksum_dir = Path(self.temporary_directory.name)
        self.tag = "v1.2.3"
        self.checksums = {}
        for index, (_, _, target) in enumerate(generator.FORMULA_TARGETS, start=1):
            asset = generator.release_asset(self.tag, target)
            checksum = f"{index:064x}"
            self.checksums[target] = checksum
            (self.checksum_dir / f"{asset}.sha256").write_text(
                f"{checksum}  {asset}\n", encoding="utf-8"
            )

    def tearDown(self):
        self.temporary_directory.cleanup()

    def test_renders_all_release_targets_and_formula_contracts(self):
        formula = generator.render_formula(self.tag, self.checksum_dir)

        self.assertTrue(formula.startswith("class MarkCli < Formula\n"))
        self.assertIn('  version "1.2.3"', formula)
        self.assertIn('  license "MIT"', formula)
        self.assertIn("  on_macos do", formula)
        self.assertIn("  on_linux do", formula)
        self.assertEqual(formula.count("    on_intel do"), 2)
        self.assertEqual(formula.count("    on_arm do"), 2)
        self.assertIn(
            '  conflicts_with "mark", because: "both install a `mark` executable"',
            formula,
        )
        self.assertIn('    bin.install "mark"', formula)
        self.assertIn(
            '    assert_match "mark #{version}", shell_output("#{bin}/mark --version")',
            formula,
        )

        for _, _, target in generator.FORMULA_TARGETS:
            asset = generator.release_asset(self.tag, target)
            self.assertIn(
                f'https://github.com/phongndo/mark/releases/download/{self.tag}/{asset}',
                formula,
            )
            self.assertIn(self.checksums[target], formula)

    def test_rejects_a_checksum_for_another_asset(self):
        target = generator.FORMULA_TARGETS[0][2]
        asset = generator.release_asset(self.tag, target)
        checksum_path = self.checksum_dir / f"{asset}.sha256"
        checksum_path.write_text(f"{'a' * 64}  another.tar.gz\n", encoding="utf-8")

        with self.assertRaisesRegex(ValueError, "expected"):
            generator.render_formula(self.tag, self.checksum_dir)

    def test_rejects_a_missing_checksum(self):
        target = generator.FORMULA_TARGETS[0][2]
        asset = generator.release_asset(self.tag, target)
        (self.checksum_dir / f"{asset}.sha256").unlink()

        with self.assertRaisesRegex(ValueError, "missing checksum file"):
            generator.render_formula(self.tag, self.checksum_dir)

    def test_rejects_non_release_tags(self):
        with self.assertRaisesRegex(ValueError, "invalid release tag"):
            generator.render_formula("nightly", self.checksum_dir)


if __name__ == "__main__":
    unittest.main()
