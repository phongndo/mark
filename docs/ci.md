# CI administration

The workflow and its local entry points are the source of truth:

- [quality.yml](../.github/workflows/quality.yml): selected validation jobs and
  the aggregate `CI gate`.
- [changes.py](../scripts/ci/changes.py): changed-path classification, covered by
  [test_ci_changes.py](../tools/test_ci_changes.py). Add a case when introducing
  a cross-component dependency or vendored asset.
- [justfile](../justfile): local `ci-*` recipes; `just ci-check` runs them all.
- [extended.yml](../.github/workflows/extended.yml): scheduled validation,
  including machine-sensitive checks kept out of deterministic PR gates.

Keep required workflows free of workflow-level `paths` filters: GitHub can
leave a skipped required workflow pending. Select jobs inside the workflow and
retain an unconditional aggregate gate.

## Repository settings

Configure a branch ruleset for `main` requiring pull requests and `CI / CI gate`.
Require `PR Template / Required PR fields` if enforcing PR metadata. Enable the
merge queue only while CI handles `merge_group` events.

Configure `release` and `nightly` GitHub Environments with the desired branch
and reviewer policy. The publishing workflows still require exact-SHA CI
qualification; an environment approval is not a replacement for it.

[Development](development.md#releases) covers release actions and the optional
Homebrew tap credential. Workflow files define schedules, platform matrices,
permissions, and packaging rather than duplicating those here.
