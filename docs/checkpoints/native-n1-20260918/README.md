# Native N1 readiness checkpoint

Status: accepted N1, deterministic native Director slice. N2 is the next
active item and remains unimplemented.

The active source is committed N1 tip
`cf91292b3bd2f17f56976efd949198f684e6f595`, whose parent/integration base is
`9bb412c21c27f5e81b0300c4b223222d483238d1`.
The N1 `vm-rust` source range has SHA-256
`41d1065972ffe75a1012218770d9c8ed4b6c24888396d67ba1a813504f491a27`.
The durable evidence root is
`/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/evidence`
and the target cache is
`CARGO_TARGET_DIR=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/target`.

Historical provenance: the original evidence was produced from isolated source
snapshot `e56266293835198bfeb31158cbbcc11233d0c20f` on branch
`codex/native-dirplayer-readiness`, with evidence and target paths under
`/private/tmp/dirplayer-native-readiness-native-stage2-n1/`. Those paths and
source identities are retained only to describe the historical origin; the
actionable receipt and reproduction paths below use the durable repository cache.

## Accepted evidence

The accepted evidence root is the durable repository cache named above.

- `integrated-lib-correction.receipt` and its paired
  `integrated-lib-correction.log`: corrected locked/offline native library
  suite, 643 tests passed, exit status 0.
- `integrated-native-probe-group-final-pass.receipt`: four native probe tests
  passed, exit status 0.
- `lifecycle-correction-1.receipt`, `lifecycle-correction-2.receipt`, and
  `lifecycle-correction-3.receipt`: three fresh OS test processes, each with
  three lifecycle iterations, all exit status 0.
- `lifecycle-correction-compare.receipt` and
  `lifecycle-correction-compare.log` establish byte-identical repeated state
  and RGBA artifacts. The native probe receipt is retained because its runtime
  and probe semantics were unchanged by the source identity correction.
- The correction lifecycle logs are `lifecycle-correction-{1,2,3}.log`; the
  correction-specific receipts and logs supersede the earlier lifecycle-final
  directories for reproduction.
- `failed-diagnostics.txt` retains the diagnostic paths from pre-correction
  attempts; those diagnostics are evidence history and were not rewritten.

The expected artifact identities are:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| state text | 402 | `d9ef2727d135f29daaea297b6db50a41de74e59da8a4bee63a0de86e29054ec0` |
| before/final RGBA | 4096 | `44ed5e870bd007e17e38119e28d150edd33828041709cc45aa8f873b5ddfb790` |
| input RGBA | 4096 | `f0e69a8d5d6c167dccda81c361d5aebdb780888cc5dc7db3bcd18297b493e59d` |

## Reproduction

Run from `/Users/clliaw/Projects/dirplayer-rs` with the recorded target directory:

```sh
mise exec -- python3 --version
mise exec -- python3 vm-rust/tests/fixtures/generate_native_director_probe.py
mise exec -- python3 vm-rust/tests/fixtures/validate_native_director_probe.py
CARGO_TARGET_DIR=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/target mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --test mod --locked --offline --no-run
CARGO_TARGET_DIR=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/target mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --test mod --locked --offline e2e::dirplayer_test_movies::native_probe -- --nocapture
env -u NATIVE_LIFECYCLE_EVIDENCE_DIR CARGO_TARGET_DIR=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/target mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --lib --locked --offline
NATIVE_LIFECYCLE_EVIDENCE_DIR=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/evidence/lifecycle-correction-1 CARGO_TARGET_DIR=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/target mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --lib player::testing::native_lifecycle_tests::native_test_player_lifecycle --locked --offline -- --exact --nocapture
NATIVE_LIFECYCLE_EVIDENCE_DIR=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/evidence/lifecycle-correction-2 CARGO_TARGET_DIR=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/target mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --lib player::testing::native_lifecycle_tests::native_test_player_lifecycle --locked --offline -- --exact --nocapture
NATIVE_LIFECYCLE_EVIDENCE_DIR=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/evidence/lifecycle-correction-3 CARGO_TARGET_DIR=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/target mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --lib player::testing::native_lifecycle_tests::native_test_player_lifecycle --locked --offline -- --exact --nocapture
```

The stable raw runtime patch identity for the committed N1 source range,
excluding documentation changes, is:

```sh
git --no-pager diff --no-ext-diff --no-textconv --binary 9bb412c21c27f5e81b0300c4b223222d483238d1 cf91292b3bd2f17f56976efd949198f684e6f595 -- vm-rust | shasum -a 256
```

It produced `41d1065972ffe75a1012218770d9c8ed4b6c24888396d67ba1a813504f491a27`.
The prior `165f...` metadata was produced with the configured external `difft`
diff driver and is superseded; this is a metadata-command correction, not a
runtime source mismatch.

## Publication and integration boundary

The accepted integration base is a 14-commit replay ending at tip `9bb412c2`;
the committed N1 source tip follows that base as `cf91292b`.
The replay excludes the local-only Ruffle gitlink `093de1f3` and retains public
Ruffle gitlink `79d1ca0f4`. Native Director code is identical across the
accepted replay boundary. Historical browser receipts describe their recorded
source snapshots and are not current public validation of the committed N1
source.

Dirty-work preservation is recorded by root commit `1d9c4408` and nested Ruffle
commit `456c2448`. At discovery, the preserved stashes were:

- root `stash@{0}` object `43af66e6982499bdbf1561ec83635170e80b1a02`,
  `On dev: preserve: pre-N1 dev changes 2026-09-19`;
- nested Ruffle `stash@{0}` object `8ab5f5c6b020e96e3993ca696d2872e2bda53e31`,
  `On (no branch): preserve: pre-N1 Ruffle changes 2026-09-19`.

## Current validation result

Direct-dev validation passed from cwd `/Users/clliaw/Projects/dirplayer-rs`
against public replay baseline
`9bb412c21c27f5e81b0300c4b223222d483238d1`. The run was performed on the
then-uncommitted bytes now captured exactly by committed N1 tip
`cf91292b3bd2f17f56976efd949198f684e6f595`, with the same range hash
`41d1065972ffe75a1012218770d9c8ed4b6c24888396d67ba1a813504f491a27`:

- `dev-integrated-lib.receipt` / `.log`:
  `CARGO_TARGET_DIR=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/target
  mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --lib --locked
  --offline`, exit 0,
  **643 passed, 0 failed**.
- `dev-native-probes.receipt` / `.log`:
  `CARGO_TARGET_DIR=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/target
  mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --test mod --locked
  --offline e2e::dirplayer_test_movies::native_probe -- --nocapture`, exit 0,
  **4 passed, 0 failed**.

The moved target and evidence were validated by the navigator; this documentation
update did not rerun them.

The generator executable identity was checked with:

```sh
mise exec -- python3 --version
```

It reported `Python 3.14.7` (with only the environment's non-fatal mise cache
warning). The output and command are retained in
`identity-correction-generator-python3.log` and
`identity-correction-generator-python3.receipt`. This explicit `python3` check
supplements the recorded toolchain identity and is the command used for the
fixture generator.

Verify every changed runtime file against the accepted source identity:

```sh
git --no-pager diff --name-only 9bb412c21c27f5e81b0300c4b223222d483238d1 cf91292b3bd2f17f56976efd949198f684e6f595 -- vm-rust |
while IFS= read -r changed_file; do
  blob_hash=$(git show "cf91292b3bd2f17f56976efd949198f684e6f595:$changed_file" | shasum -a 256 | cut -d ' ' -f 1)
  printf '%s  %s\n' "$blob_hash" "$changed_file"
done
```

The twelve accepted per-file hashes are:

```text
7f0929bfdb009960946ffa62185f6e12dba62b8b34486df4163a2e250c2d325d  vm-rust/src/player/commands.rs
fdd007d644a89ce6d6e2995dbf29fc12a271d992baed5b9cba47c4addc911ce6  vm-rust/src/player/handlers/datum_handlers/timeout.rs
4b7d0d66f5a7bf0cef69f85d9c1d7597da923c7cce3cc2014afdc9d145600c98  vm-rust/src/player/mod.rs
f0382dda1f57a0a69a36e27c4bea519edfecea7568271378fa2e37a31897517f  vm-rust/src/player/session.rs
5240c3b915e59b76ae4975ff03e5faab6c067eac84074b12ccddcda625dccc98  vm-rust/src/player/testing.rs
2447c491beeb6c2ef097b2e1c2e2550f8b8c302ce7423e7e40c2f493bf991a82  vm-rust/src/player/timeout.rs
f30e384f53baeae2c53a05ce8672d2b19082f561a2ff5e1c2ca0fc56d9c264cc  vm-rust/src/rendering.rs
e9552f1bf5e5ef8434f205eea14d19803a8c012420b9e79456665079354111f3  vm-rust/tests/e2e/dirplayer_test_movies/native_probe.rs
8581678f2c21bb2a233fae3d545e9586cd954d16d40b4309f35cdae8df23cf8d  vm-rust/tests/fixtures/generate_native_director_probe.py
88b08a92b01e66dddd18b75be665dfaf0bb3b1de6967987d42a8fd082b1b516d  vm-rust/tests/fixtures/native_director_probe.dcr
d0cdc7256f26a518c29afca29a3aba0064b7c35488683804b1d15d7cb8864d7c  vm-rust/tests/fixtures/native_director_probe_provenance.md
a45a970a872242618d2b1a0d5bc029fd8eb205f76fd8c52522ccf3127269dd7d  vm-rust/tests/fixtures/validate_native_director_probe.py
```

The documentation files are intentionally excluded from the runtime patch
identity. Validate both repositories and the isolated repository's untracked
receipt with:

```sh
git diff --check
git -C /Users/clliaw/Projects/childhood-redux diff --check
git diff --no-index --check /dev/null docs/checkpoints/native-n1-20260918/README.md
```

The final command exits 1 for an additions-only no-index diff; acceptance
requires that it emit no whitespace diagnostics.

## Toolchain and platform

The recorded identity receipts report:

- mise `2026.4.6 macos-arm64`;
- rustc `1.98.1 (48a229cea 2026-09-01)`, host `aarch64-apple-darwin`;
- cargo `1.98.1 (797e8a9bc 2026-08-05)`;
- Python `3.14.7` from the recorded `mise exec -- python3 --version`
  generator-executable check;
- `macOS-26.7-arm64-arm-64bit-Mach-O`.

## Limits and exclusions

This receipt accepts only the synthetic Director slice. It does not claim the
N2 JSON-lines worker, browser replacement, game acceptance, Spybot, Battalion,
Flash, audio, Bevy presentation, native worker process protocol, broad legacy
cleanup, global/ambient adapter removal, or in-process multi-session ownership.
The three-process lifecycle evidence demonstrates fresh-process repeatability;
it does not establish worker storage isolation or concurrent worker acceptance.
Wall-derived input metadata is not simulation time, and no wall-clock sleep
drives advancement. Existing mutable global and ambient adapters remain a known
single-session limit. Unsupported behavior remains explicit rather than being
treated as a pass.
