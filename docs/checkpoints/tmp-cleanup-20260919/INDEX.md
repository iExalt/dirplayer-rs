# Dirplayer temporary cleanup — 2026-09-19

Removed the inventoried temporary worktrees, generated builds, duplicates and historical logs using `rmz` for directories. The pilot inventory estimated about 4.96 GiB removed; this is an estimate, not a filesystem free-space measurement. See [deletion manifest](MANIFEST.md).

Retained checkpoint archives contain historical readiness, timer and native-probe evidence. The ownerless score snapshot, accepted overlay and untracked fixture inputs remain available for inspection without keeping whole temporary builds.

## Recovery limitation

The cleanup removed dirty temporary trees before validating the captured diffs. Three captures used the external `difft` renderer rather than Git patches. They are retained as compressed `*.review.txt.gz` files for inspection, **not applyable patches**. Exact reconstruction of every uncommitted byte has not been established; original temporary trees are gone. Their manifests record affected paths and original base commits. Do not treat these captures as complete backups.

Two missing publication commits only changed the Ruffle URL in `.gitmodules`; their changes were reconstructed from the complete surviving comparison against their available parent commits, and checked with `git apply --cached --check`. This recovers their changes, not their original commit objects. The childhood-redux commit patch was regenerated from its surviving Git object using `--no-ext-diff --binary`.

Historical commands in earlier receipts retain their original paths and are not claims that those temporary locations still exist. No runtime/source behavior was changed and no runtime tests were rerun for this archival cleanup.

## Retained file checksums

SHA-256 values below describe current retained bytes, not proof of full recovery.

- `docs/checkpoints/native-ownership-20260917/artifacts/dirplayer-score-ownerless.rs` — 377653 bytes; SHA-256 `e2f8a97c098db1de9e5b01a784715999c2a7688e75a6ecc29de7a9e4e64b90d7`
- `docs/checkpoints/native-ownership-20260917/artifacts/dirplayer_test_movies/mod.rs` — 96 bytes; SHA-256 `28f915da7976a8c9a5d3f089567343b33da787ff3f03913b2fd57ea71056069f`
- `docs/checkpoints/native-readiness-20260918/artifacts/accepted-overlay-62/dirplayer-js-api/index.d.ts` — 13491 bytes; SHA-256 `35555a619ae5e910622b223fdf234e09858584f7623f3bad888fbd5ca924a66f`
- `docs/checkpoints/native-readiness-20260918/artifacts/accepted-overlay-62/dirplayer-js-api/index.js` — 48226 bytes; SHA-256 `b5a5c9ff97f24d713a5b343bddb1f4f4604ac6256b76c8a04381e182c748e715`
- `docs/checkpoints/native-readiness-20260918/artifacts/accepted-overlay-62/dirplayer-js-api/package.json` — 237 bytes; SHA-256 `703e04f3e1e2aa9ba555655bfd851311aa96bc0c373b82473d190b266398c4ce`
- `docs/checkpoints/native-readiness-20260918/artifacts/accepted-overlay-62/vm-rust/tests/browser_templates/dirplayer-js-api.js` — 22001 bytes; SHA-256 `fe596df54c8361a42ba0cac576aebd7617040faa6beb7a5b3eac9a244855d74e`
- `docs/checkpoints/native-readiness-20260918/artifacts/accepted-overlay-62/vm-rust/tests/e2e/dirplayer_test_movies/nested.rs` — 3145 bytes; SHA-256 `bd61e02b12205d41d2bc1ddafc0ad3043708e1c51920ca2999a2b7255f9e4ec8`
- `docs/checkpoints/native-readiness-20260918/native-probe/artifacts/dirplayer-native-probe-evidence-corrected-20260918.tar.gz` — 409647 bytes; SHA-256 `f032da61eb102ebc501808d00a74ae9e2d1216315ffcdcce07446db538c6d5de`
- `docs/checkpoints/native-readiness-20260918/readiness-baseline/artifacts/dirplayer-native-readiness-baseline-evidence-20260918.tar.gz` — 1606119 bytes; SHA-256 `cadc800d2b1653f3b851a96cca62bf6fa7e4e67d278cb55e53ae76a80d397706`
- `docs/checkpoints/native-readiness-20260918/timer-runtime/artifacts/dirplayer-native-readiness-timer-runtime-logs-20260918.tar.gz` — 1349726 bytes; SHA-256 `1fa550cf51bcbee5de218887292b903305a32b2dd549d719b58e5d90ace8cd66`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/commit-childhood-redux-16eac89.diff` — 36385 bytes; SHA-256 `d1e78c40370b6845956d8908f14a0c904298f0b106155d96ea1e366e3880b02c`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/commit-childhood-redux-16eac89.metadata.txt` — 791 bytes; SHA-256 `b9ea56a33379a7c6367fc70aa9520484a9e8886ae0afa3c94fac35a99f2e9bdf`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/commit-dirplayer-rs-25f40d1.diff` — 311 bytes; SHA-256 `fb9a7e7dec28b0ee5f56b120386b7313762789e1e6e88d7311f7ad7217adc167`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/commit-dirplayer-rs-25f40d1.metadata.txt` — 974 bytes; SHA-256 `489551ce5f6c26a08a13488fd1686b28c778fd6880bbe3019a624161de993204`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/commit-dirplayer-rs-6b24b6b.diff` — 311 bytes; SHA-256 `fb9a7e7dec28b0ee5f56b120386b7313762789e1e6e88d7311f7ad7217adc167`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/commit-dirplayer-rs-6b24b6b.metadata.txt` — 983 bytes; SHA-256 `9bd3e8d046ff13ddaaabf865dc166b7086ccccc2708bf65eea6eacf6f31faf4a`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/dirty-native-readiness.manifest.txt` — 3768 bytes; SHA-256 `49d90b5ea2d78b4cdb0218c8e1d7edad07a29bc43cd97c594972151d0718ca65`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/dirty-native-readiness.review.txt.gz` — 246179 bytes; SHA-256 `c257cdb4dbf4fb7b73685e9beab136b631169452bf04a4e912bc8b3a585b3678`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/dirty-publication-parent-clean.manifest.txt` — 22097 bytes; SHA-256 `3d72a38b1ad11a76466bb40a5543993f4898b8940ec4c28f7a5ee18f5464eeec`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/dirty-publication-parent-clean.review.txt.gz` — 1156022 bytes; SHA-256 `7918f5119ec0f4134741ef066915bfc5b74547f1b15fc255a850f6b4c29a454a`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/dirty-semantic-apply.manifest.txt` — 508 bytes; SHA-256 `b6b81188b0ba52c35ddecf62916b949c7b4e410f994b27ca599ea0a2d039b6c2`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/dirty-semantic-apply.review.txt.gz` — 5866 bytes; SHA-256 `7699b409f79d98bd88c4bd9918998d95f9d7a6e40e7cee2b58e4b83c7cfc8957`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/flash_lingo_callback_fixture_provenance.md` — 3329 bytes; SHA-256 `2571c52363b6f83c96101a14877da79d1e81641af86b74574a95ef276d9ae766`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/flash_lingo_callback_probe.swf` — 1167 bytes; SHA-256 `e5d52be9cab553c8342e160ff119b92cbd60425141717fc8a6b88ebfe6abac7f`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/flash_mouse_a.swf` — 1900 bytes; SHA-256 `2112fdcd4153807408c02d3b72aeae08ca376787ca8a5a32aa98f5f6edf9c44d`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/flash_mouse_b.swf` — 1900 bytes; SHA-256 `60c316ccea5c30301d6244d3123331b583bde6451498dedcd71897cff982f118`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/flash_mouse_reentry.swf` — 2153 bytes; SHA-256 `26ecc422a5ad7d7ac7a392a58e4e9f61ef2f1ed67c9bb45b1d56dd27c030f268`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/flash_pointer_fixture_provenance.md` — 5024 bytes; SHA-256 `fd1718256244ea50c56e1801b084150d51befca140cdc772657327e2d2e39600`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/generate_flash_lingo_callback_fixture.py` — 8692 bytes; SHA-256 `73c57026c92ef5ed850ac18010afbc0308b5bd0b050c6405dfe5d6d2f0f942ee`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/generate_flash_pointer_fixture.py` — 7596 bytes; SHA-256 `cf65425b500eeac40bcf344efe988d5e2e0e7784df73bb4815e71590322c9c77`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/generate_nested_flash_fixture.py` — 10676 bytes; SHA-256 `aa3bd08bff9c15b27e44150e429b27119c357a344df8d5004ec86046ac8d8be8`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/nested_flash_a.dcr` — 809 bytes; SHA-256 `60efa5531718c0c36a47bcecc4f18ce94142510aa5e85b26daa66c45422e1bfa`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/nested_flash_a.swf` — 88 bytes; SHA-256 `ea1e73bfee01e270f65d65ba558e98dc7416854cd18195e99c7748ae12f45e25`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/nested_flash_b.dcr` — 809 bytes; SHA-256 `1a7fcc3bac605b213bad131ea41c771bb67f9f53e8565e49d7231b055f167ffb`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/nested_flash_b.swf` — 88 bytes; SHA-256 `65c3caf790f88a7460eafbd9ee0fb83ceaa8160be751203d3034b2d8a64b7c8e`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/nested_flash_bad.dcr` — 809 bytes; SHA-256 `2433bd5f454bce3a0a0efc5071811f6bbb15ae0684193a6102fb643122b64dc3`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/nested_flash_fixture_provenance.md` — 2241 bytes; SHA-256 `9fdc2fc1784042cf1ca9e27c1b6a2b0d5cc533752a4fb0b66c2b1dedef7ee469`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/validate_flash_lingo_callback_fixture.py` — 14585 bytes; SHA-256 `354151c2bea1382d5481b9210feefde0426280383a115e737a144d60f86f2d58`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/validate_flash_pointer_fixture.py` — 9833 bytes; SHA-256 `523882fc5c37692fd0d9be9fee7d09626c2a1735d22caa1f0abb070c26a19d3e`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness/vm-rust/tests/fixtures/validate_nested_flash_fixture.py` — 6410 bytes; SHA-256 `bbeb377ec0cd00a1464fc19dc26f90901daf5c451e7df6b5a9af0ddc95903291`
- `docs/checkpoints/tmp-cleanup-20260919/recovery/untracked-native-readiness.manifest.txt` — 3027 bytes; SHA-256 `4bc7087ef3ca3f915ed25fdf73965f0c225a8f8bef1808c9da73b1d6cd913ed4`
