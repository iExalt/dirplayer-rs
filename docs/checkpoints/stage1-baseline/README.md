# Stage 1 focused baseline evidence

`manifest.json` records historical source revisions and SHA-256 digests of retained
receipts, harnesses and license texts. `audio-baseline-297da4a.json` is copied from
the original valid JSON run log, rather than the malformed reconstructed cache
receipt (which ends with a literal `\\n`). The old cache files are unchanged.

The eight-case audio result belongs to `297da4a410495a1116e2c1b93ee57f9f1b5c8d79`.
The shape log reports one pass and zero failures, but its exact working-tree
source was not retained; it is **not** proof that revision passed. Neither result
certifies the current Stage 2 runtime. The original generated frozen browser
artifacts are absent. A retained raw WASM with the same filename regenerated to
different hashes and was rejected as a recovery of the frozen artifact.

Ruffle's `LICENSE.ruffle.md` is extracted from the pinned submodule commit and
includes its MIT/Apache licensing terms. `LICENSE.dirplayer` is the main source
license from the pinned baseline commit. Licenses do not grant distribution rights
to third-party movie fixtures; no movie payload is included here.

## Historical-source reproduction

Use a new source-only directory, preserving the current checkout. Run commands
through the repository's mise environment (Rust 1.98.1, Node 24.14.0,
wasm-bindgen 0.2.108). The historical audio harness must be copied into that
checkout's `scripts/` directory with `test-sound-playback.mjs`; it uses the
pre-ownership API and cannot be replaced by the current BrowserPlayerHandle
harness. The preparation script archives exact Git source, reuses installed
JavaScript dependencies, and records the retained Ruffle bundle's files.

```sh
# From dirplayer-rs; use absolute paths for external directories.
mise exec -- python docs/checkpoints/stage1-baseline/prepare-historical.py \
  /absolute/path/to/new/source \
  /absolute/path/to/verified/historical/public/ruffle

# Share an existing Cargo cache; do not copy targets into source snapshots.
CARGO_TARGET_DIR=/absolute/path/to/shared/target mise exec -- cargo build \
  --manifest-path /absolute/path/to/new/source/vm-rust/Cargo.toml \
  --test mod --target wasm32-unknown-unknown --release --locked --offline \
  --message-format=json-render-diagnostics > /absolute/path/to/compiler-artifacts.jsonl

# Continue only after Cargo succeeds. Select its reported test artifact.
BASELINE_WASM=$(mise exec -- python docs/checkpoints/stage1-baseline/select-test-artifact.py \
  /absolute/path/to/compiler-artifacts.jsonl)
mise exec -- wasm-bindgen "$BASELINE_WASM" \
  --out-dir /absolute/path/to/new/source/vm-rust/target/browser_runner --target web
mise exec -- python docs/checkpoints/stage1-baseline/prepare-historical.py \
  /absolute/path/to/new/source --runner-only

CHILDHOOD_REDUX_ROOT=/absolute/path/to/childhood-redux \
PLAYWRIGHT_BROWSERS_PATH=/absolute/path/to/playwright/cache \
NATIVE_BEVY_SOURCE_REVISION=297da4a410495a1116e2c1b93ee57f9f1b5c8d79 \
NATIVE_BEVY_AUDIO_BASELINE=/absolute/path/to/new/audio-result.json \
mise exec -- node /absolute/path/to/new/source/scripts/run-historical-audio-baseline.mjs
```

The preparation script changes only the archived harness's Ruffle revision lookup
to the extracted commit literal: an archive has no Git metadata. It records both
harness hashes, so this adaptation cannot be confused with the original script.
Builds need the historical locked crates installed locally for `--offline`; if
missing, explicitly fetch dependencies online before retrying. Do not repair
historical runtime code just to turn this receipt green.

For shapes, provide the authorized fixture at
`source/public/dcr_dirplayer_test_movies/D8_5_00001_shapes_1.dcr`, initialize the
historical snapshot reference submodule, and run the historical
`E2E_FILTER=dpt_shapes npm run e2e-test-browser -- --reporter=line` under mise.
That harness builds and regenerates its own browser runner. The fixture is not
currently present locally. The repository CI obtains it from `TEST_ASSETS_URL`,
a GitHub Actions secret; this package neither retrieves nor contains that secret.
The rest of the licensed-movie campaign remains explicitly excluded.

## Fresh reproduction evidence

`reproduction-preparation.json` identifies the isolated source and reused bundle.
`compiler-artifact.jsonl` records the exact successfully compiled test module.
`reproduction-run.json` records the build and excluded attempts. The first audio
attempt accidentally selected a stale matching-named artifact and timed out;
its caller-supplied source label is **invalid** and that receipt is retained only
as rejected diagnostic evidence. The corrected run uses the Cargo-reported file.
`fixture-files.json` hashes every file in the served Spybot fixture directory.

The corrected historical-source audio run passed all eight cases with no script
errors. Its new generated artifact hashes are in `audio-reproduced-297da4a.json`;
they are separate from the original frozen receipt. The entire served fixture
directory was unchanged after the run. No fresh shape run is claimed.
