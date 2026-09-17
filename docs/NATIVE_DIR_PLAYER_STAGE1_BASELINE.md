# Native DirPlayer Stage 1 baseline

This checklist records the Stage 1 baseline for the `native-bevy` branch before
the shared-runtime ownership migration. It is tied to source revision
`297da4a410495a1116e2c1b93ee57f9f1b5c8d79` and the Ruffle submodule revision
`79d1ca0f45d79e28c3a0658bbc6b8430d26ef308`. The ownership classification is
maintained separately in [NATIVE_OWNERSHIP_INVENTORY.md](NATIVE_OWNERSHIP_INVENTORY.md).

## Retained evidence and current reproducibility

The durable [Stage 1 evidence package](checkpoints/stage1-baseline/README.md)
retains the original valid eight-case audio JSON, complete focused shape log,
version-compatible historical audio harness, license texts, and a hashed manifest.
It provides an isolated historical-source reproduction recipe. The generated
frozen browser artifacts are no longer available; a matching-named raw candidate
regenerated to different hashes and was rejected. The shape movie is currently
missing locally, and its retained passing log has no exact working-tree source
manifest. These historical results do not verify the current runtime.

A fresh isolated build of the historical source passed all eight audio cases
with no script errors; see
[audio-reproduced-297da4a.json](checkpoints/stage1-baseline/audio-reproduced-297da4a.json).
The release build passed in 2m35s. Cargo's successful artifact receipt selected
the generated WASM, and all served fixture files remained unchanged. Generated
hashes differ from the original frozen artifact. The retained first failed
attempt is explicitly excluded because it accidentally selected a stale file.

The commands below target the **current checkout**, not the historical baseline.
The browser harness generates the JavaScript/WASM pair and API bridges required
by the audio runner; direct `cargo build` alone does not prepare those inputs.
Both current runners honor `BROWSER_RUNNER_DIR`, falling back to
`CARGO_TARGET_DIR/browser_runner` or `vm-rust/target/browser_runner`, including
`.env` configuration with process environment taking precedence. Use the same
settings for both commands. The current audio harness requires the current
`BrowserPlayerHandle` API; use the retained historical harness for `297da4a`.

## Historical checklist (2026-09-14)

- [x] Capture the baseline from the clean `native-bevy` branch at `297da4a`,
  before the ownership pilot's subsequent runtime edits.
- [x] Initialize the Ruffle submodule recursively and verify its pinned
  revision (`79d1ca0`).
- [x] Install the existing root and Ruffle web dependencies with `npm ci`;
  lockfiles remain unchanged.
- [x] Build and copy the Ruffle web bundle. The repository retains
  `public/ruffle/LICENSE_APACHE` and `public/ruffle/LICENSE_MIT`.
- [x] Build the current release browser runner with the pinned Rust and
  `wasm-bindgen` toolchain.
- [x] Run the available focused browser regression: `dpt_shapes`, 1 passed,
  0 failed.
- [x] Run the eight-case `testSoundPlayback` regression against the recovered
  Spybot fixture: 8 cases passed after limiting instrumentation to Director's
  `AudioContext`.
- [ ] Run the complete browser movie campaign. It is outside this baseline's
  available fixture scope: the destination checkout has no licensed movie
  payloads under `public/`, so only `dpt_shapes` was run.
- [x] Run the ownership scanner unit tests: 3 tests passed.
- [ ] Make the ownership gate green. It currently exits 1 as expected before
  the Stage 2 ownership refactor; the scanner result is not a stage-completion
  claim.

## Current-checkout commands (not a fresh runtime verification)

These commands require the shape fixture at
`public/dcr_dirplayer_test_movies/D8_5_00001_shapes_1.dcr`, which is currently
absent. Supply it through the authorized fixture setup before running shapes.
The historical checklist above records three scanner tests; the current scanner
test suite is tracked separately in the ownership inventory.

Run these commands from the repository root. The defaults below point at the
verified Chromium 1217 cache in a sibling `childhood-redux` checkout; override
the two variables when the checkouts are elsewhere. The audit pilot has also
seeded and launch-verified the repo-local `.cache/native-bevy/playwright`
cache for the mise tasks.

```sh
CHILDHOOD_REDUX_ROOT="${CHILDHOOD_REDUX_ROOT:-../childhood-redux}"
PLAYWRIGHT_BROWSERS_PATH="${PLAYWRIGHT_BROWSERS_PATH:-$CHILDHOOD_REDUX_ROOT/.cache/spybot-playtest/browsers}"

mise exec -- git submodule update --init --recursive ruffle
mise exec -- npm ci --ignore-scripts --no-audit --no-fund
mise exec -- npm ci --prefix ruffle/web --ignore-scripts --no-audit --no-fund
mise exec -- npm run build --prefix ruffle/web
mise exec -- npm run copy-ruffle

mise exec -- cargo build --manifest-path vm-rust/Cargo.toml \
    --test mod --target wasm32-unknown-unknown --release

mise exec -- env \
  PLAYWRIGHT_BROWSERS_PATH="$PLAYWRIGHT_BROWSERS_PATH" \
  E2E_FILTER=dpt_shapes npm run e2e-test-browser -- --reporter=line

mise exec -- env \
  PLAYWRIGHT_BROWSERS_PATH="$PLAYWRIGHT_BROWSERS_PATH" \
  CHILDHOOD_REDUX_ROOT="$CHILDHOOD_REDUX_ROOT" \
  NATIVE_BEVY_SOURCE_REVISION="${NATIVE_BEVY_SOURCE_REVISION:?set to the source revision used for this build}" \
  node scripts/run-native-bevy-audio-baseline.mjs

mise exec -- python scripts/audit-runtime-ownership.py --format json
mise exec -- python scripts/audit-runtime-ownership.py --check
mise exec -- python scripts/test_audit_runtime_ownership.py
```

The audio runner writes a timestamped JSON result under `test-results/` and
requires one uniquely stemmed `mod-*.js` and `mod-*_bg.wasm` pair in the
resolved browser runner directory. Set `NATIVE_BEVY_AUDIO_BASELINE` to retain a
stable filename in a local evidence directory. Its fixture is
`resources/spybot/spybot-nightfall-incident.dcr` from the configured
`CHILDHOOD_REDUX_ROOT`; the runner records that movie's SHA-256. The required
`NATIVE_BEVY_SOURCE_REVISION` identifies the source used to produce the frozen
artifact and prevents a mutable checkout `HEAD` from being mistaken for its
provenance.

The Stage 1 browser baseline uses `npm run e2e-test-browser` for the existing
test harness. The audio baseline uses the direct release runner command above
and the existing `scripts/test-sound-playback.mjs` helper. The helper's
Director filter compares each source's actual `AudioContext` identity with
`window.getAudioContext()` and restores the Web Audio prototype after the run;
this prevents Ruffle's own PCM source from being counted as Director audio.

## Baseline provenance

The pinned mise toolchain used for the checks is:

| Input | Value |
| --- | --- |
| Node | `v24.14.0` |
| npm | `11.9.0` |
| Rust/cargo | `rustc 1.98.1`, `cargo 1.98.1` |
| Python | `3.14.7` |
| wasm-bindgen | `0.2.108` |
| Playwright | `1.59.1` |
| Chromium | `147.0.7727.15`, Playwright revision `1217` |

The frozen baseline runner generated `mod-2671e42b945ed1b1.js` and
`mod-2671e42b945ed1b1_bg.wasm`:

| Artifact | SHA-256 |
| --- | --- |
| browser runner JavaScript | `6b65582ada1371cf5094b7d0a64b4a486b1612e5d4b2467beb4d24ae6f4fc5c4` |
| browser runner WebAssembly | `5031199e4f52ab9a11065077c12f864214e4f84f3966681b1b0f0c3e27c6eea5` |
| Spybot baseline movie | `ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77` |
| `public/ruffle/dirplayer_ruffle.js` | `704f8d3004e402b46a8781496be8be3bbb7f334e3327e6085ac36b1a9aa206b5` |
| root `package-lock.json` | `d245108de43a4e0ae27c811cab0d1957c7774eb426b8568bfee2242bf58c5d44` |
| `ruffle/web/package-lock.json` | `0170a4c82e87c958c84274d9e130670b85d4708e623c1e1ffcdfc158a04b497a` |
| `vm-rust/Cargo.lock` | `0be0d949624679b5d0f4bfa1d0ec65dd59b7bbeabdbfecc91438fa3248d0f641` |
| `ruffle/Cargo.lock` | `5198749e1f55122a5278b1163d33c82ed1c19bbe08e0913945ccde679218f586` |
| `scripts/run-native-bevy-audio-baseline.mjs` | `b30661b98e3c8e092d26b99ae119a9b99724d26928ca4e4c8bc1b384d113dfc6` |
| `scripts/test-sound-playback.mjs` | `340dd42f78a3f80bfdf1ff43d34b102b799481f3a334186f0ec10ee68a0ba9c6` |

These hashes describe the frozen pre-ownership-edit baseline artifact; they
must not be interpreted as hashes of a later `HEAD`. The original focused
shape `.last-run.json` was overwritten by a later browser run. The durable
rebuilt shape result is recorded in
`.cache/native-bevy/evidence/native-bevy-browser-shapes-20260914T051042Z.log`
and is labeled as a current working-tree verification. The passing audio result
from the frozen artifact was reconstructed from its retained run log and
archived as `.cache/native-bevy/evidence/audio-baseline-297da4a.json`.
Generated targets, `public/ruffle`, and `test-results` are local build/evidence
outputs; they are not source changes.

The audio run that preceded the context filter observed a Ruffle source with
duration `0.042666666666666665` seconds (2048 frames at 48 kHz), which caused a
false queue assertion. Its failed JSON was overwritten during later runner
cleanup and is not available for durable reconstruction. The filtered run
records all eight expected Director cases, including queue and playlist rates
`[0.5, 2]` and distinct durations `[0.72, 1.116]`.

The release build completed with existing compiler warnings in the baseline
source. This baseline tooling did not edit runtime, Cargo source, package
manifest, or license files; subsequent ownership-pilot runtime edits are
visible in the shared worktree and are intentionally preserved.

## Available mise tasks

The pinned task definitions expose the baseline and ownership checks directly:

- `setup:browser` — install JavaScript dependencies, Chromium, and the pinned
  WebAssembly target.
- `baseline:browser:shapes` — run the `dpt_shapes` command with the verified
  Playwright browser path.
- `baseline:audio` — run `scripts/run-native-bevy-audio-baseline.mjs` and
  preserve its nonzero result if a regression occurs.
- `audit:ownership` — scanner report.
- `check:ownership` — scanner gate; expected nonzero until Stage 2/3 removes
  the reported mutable singleton and legacy-accessor findings.
- `test:ownership-audit` — scanner unit tests.
- `build:vm` — build the regular VM WebAssembly package with the pinned
  `wasm-pack` tool.

The pinned mise configuration also provides `wasm-pack 0.15.0` for the
existing `build:vm` task. This baseline's recorded release build used direct
`cargo build` plus pinned `wasm-bindgen`; a later `build:vm` run must record its
own generated artifact hashes.
