# Native DirPlayer cumulative readiness baseline

This checkpoint starts from the cumulative baseline rebuilt from root `fa80cb175625ac0e72022ff399247623f66b00cd` and the latest sprite-variable acceptance overlay. It then reapplies the complete preserved BrowserOwner/timer candidate on an isolated branch.

## Exact source boundary

- Root base: `fa80cb175625ac0e72022ff399247623f66b00cd`
- Canonical branch: `codex/native-dirplayer-readiness`
- Accepted overlay archive: `/Users/clliaw/Projects/.dirplayer-work/sprite-variable-stage2/accepted-source-overlay.tar.gz`
- Overlay archive SHA256: `31c2b0f3ceb98833beb9211aca0b33379b031ce6c2719198bede7b14ff80abb1`
- Overlay entries: 62 total, 51 root and 11 nested
- Extracted raw entry/hash stream SHA256: `fb45f80dabb748c5b47d740b403f97145e8aeb575c9105115322a3ecb2e7773c`
- Nested accepted commit: `093de1f3c356e28b7506ce2712ab48ceaecf9af8`
- Nested 11-entry stream SHA256: `290f9cb35fda413f81bb37ee76195a700115943c651353d8577fadebf3d03c2e`

`accepted-overlay-62.sha256` is the latest sprite acceptance manifest derived mechanically from the archive. Every extracted file hash matched its tar member hash (`62/62`); every nested overlay file matched commit `093de1f3` (`11/11`).

## Rejected reconstruction history

The earlier reconstruction is preserved only for audit on branch
`codex/native-dirplayer-readiness-rejected-20260918` at `3ff6ef39`. Its
baseline commit `1aae34c5`, timer source commit `d5bce000`, and reconciliation
receipt `3ff6ef39` are rejected. The baseline replaced accepted mixed-file
identities with older pre-timer variants: for example, it dropped the accepted
shared session allocator and callback-origin fencing. The current baseline was
rebuilt independently from the literal 62-file archive; no rejected source was
carried forward.

## Preserved original snapshot

- Original root: `/Users/clliaw/Projects/dirplayer-rs`
- Original nested Ruffle base: `79d1ca0f45d79e28c3a0658bbc6b8430d26ef308`
- Original root diff SHA256: `15588ab0bdbda53db608b9b60bf06dde21bb55836b167f68fc4480bc3158593e`
- Original nested diff SHA256: `f928008992e7822afac077f74055b3a217ff424c78bf4400fb0aeafbcca5b11b`
- Untracked file count: 44
- Untracked manifest SHA256: `ab8d02f8a81041cffb2240e2af9278b7a4890868d7354ff6d231d489f0d7a49b`
- Untracked archive SHA256: `4e82daeed4a0973656180b0317fa8bb0cf669b179cb1de89ac0089e3d4b12f6d`
- Standalone `/Users/clliaw/Projects/ruffle` was not modified.

## Cross-checkpoint provenance

The latest sprite overlay supersedes same-path raw identities from earlier
score and callback checkpoints. `checkpoint-identity-reconciliation.json`
records the exact hashes from each checkpoint's `acceptance.json`, the current
cumulative hash, and whether the later checkpoint is byte-identical or requires
a semantic-retention check. Superseded critical paths are retained by
source-only checks in `semantic-retention-checks.json`:

- shared `allocate_session_id` and session allocator
- callback-origin owner and generation fences
- `FlashCallbackArgs` transport forms
- evaluator/Flash pump and teardown paths
- sprite binding plus get/set and `SpriteAsync` paths

No earlier same-path hash is required to equal the later sprite version when the later overlay explicitly supersedes it.

## BrowserOwner/timer source reconciliation

- Corrected cumulative baseline receipt: `d4d03572`
- Corrected cumulative source tree: `847ade6f`
- Mechanical timer-owned formatting: `e374d357`
- Complete BrowserOwner/timer source: `d79512f3`
- Mechanical manifest: `timer-style-9.sha256`
- Reconciled source manifest: `timer-source-21.sha256`
- Scope and equivalence evidence: `timer-source-reconciliation.json`

The mechanical commit contains exactly nine reviewed timer-owned Rust files.
Each file is byte-identical to deterministic `rustfmt 1.9.0` output from
`d4d03572` with edition 2024, `ruffle/rustfmt.toml`, `skip_children=true`, and
import/module reordering disabled. Formatting was applied only in the isolated
worktree and temporary proof copies; the preserved original source was not
modified.

The source commit contains exactly 21 paths. Eleven are byte-identical to the
preserved original candidate. Six Rust paths are byte-identical after applying
the same deterministic formatter to temporary copies of the original. The
remaining four Rust paths report no syntactic changes in Difftastic after that
normalization. This accounts for the complete preserved candidate, including
the BrowserOwner rename, owner-qualified timer scheduling and clearing, timer
incarnations, synchronous `TimeoutRef` assignment, timeout drains,
`BrowserPlayerHandle` teardown, manager/frontend diagnostics, and the nested
fixture.

Eight formatter-only quarantine paths remain byte-identical to `d4d03572`.
The nested Ruffle source remains at accepted commit `093de1f3`, and the
standalone Ruffle checkout remains unchanged.

## Gate status

The cumulative baseline and complete BrowserOwner/timer source are ready for
parent review. No build, runtime test, or network operation was run. Focused
native timer and browser lifecycle verification remains pending.
