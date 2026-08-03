---
status: accepted
date: "2026-07-29"
category: Operations
applies_to:
    - docs/operations/src-vm.md
    - Cargo.toml
priority: invariant
---

# Deploy only pinned release tags

## Decision

The SRC catalog item provisions the campaign machine by `git checkout
<pipeline_git_ref>`, where the ref is an annotated release tag (currently the
`v0.x` series), never a branch. Shipping code to a campaign machine is a
promotion, not a pull.

## Guidance

- Promotion is five explicit steps, always in order: (1) merge to `main`,
  (2) cut an annotated tag with release notes (`git tag -a vX.Y.Z`), (3) push
  the tag, (4) bump the catalog item's `pipeline_git_ref` to that tag, (5)
  delete-and-relaunch the workspace. Never point `pipeline_git_ref` at a
  branch (`main` included) — a campaign machine that rebuilds mid-run
  (crash, restore-from-storage, relaunch) must reproduce byte-equivalent
  behavior, which only a fixed tag guarantees.
- The manual `git pull` + rebuild path in `docs/operations/src-vm.md` is a
  dev/emergency escape hatch only — it diverges the running binary from the
  pinned tag and must be labeled as such wherever it's documented; it is
  never the production update procedure.
- Every epic close-out checklist asks explicitly: "does this need a release
  tag?" — an epic that changes shipped behavior and skips the tag leaves
  `pipeline_git_ref` pointing at stale code with no signal that it's stale.
- `Cargo.toml`'s `version` is the human-readable half of the same promotion:
  bump it as part of cutting the tag, not independently of it.

## Why

2026-07-29 incident: the campaign workstation faithfully rebuilt
`pipeline_git_ref = v0.2.0-rc1`, which was pre-Epic-3 code, while `main` was
four epics ahead. The provisioning was not broken — the *promotion habit*
was: nobody had cut and pushed a tag past `v0.2.0-rc1`, so "ship to
production" had silently become "hope the machine still has whatever was
built by hand last time." Pinning to a tag only enforces reproducibility if
someone treats moving the pin as a required, explicit release step —
otherwise a machine that rebuilds runs code nobody chose on purpose.
