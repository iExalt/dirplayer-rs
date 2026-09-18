# Native DirPlayer cumulative readiness baseline

This is the fresh cumulative baseline rebuilt from root `fa80cb175625ac0e72022ff399247623f66b00cd` and the latest sprite-variable acceptance overlay. Timer/BrowserOwner reapplication is intentionally pending this baseline review.

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

The latest sprite overlay supersedes same-path raw identities from earlier score/callback/evaluator checkpoints. `checkpoint-identity-reconciliation.json` records each prior identity and whether it remains byte-identical or is superseded. Superseded critical paths are retained by source-only checks in `semantic-retention-checks.json`:

- shared `allocate_session_id` and session allocator
- callback-origin owner and generation fences
- `FlashCallbackArgs` transport forms
- evaluator/Flash pump and teardown paths
- sprite binding plus get/set and `SpriteAsync` paths

No earlier same-path hash is required to equal the later sprite version when the later overlay explicitly supersedes it.

## Gate status

The cumulative baseline is ready for parent review. No timer delta has been reapplied. No build, runtime test, network operation, or target formatter was run.
