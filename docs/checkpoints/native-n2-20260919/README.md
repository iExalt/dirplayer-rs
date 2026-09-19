# Native DirPlayer N2 acceptance

N2 is accepted on 2026-09-19 for the synthetic `native_director_probe.dcr`
through the childhood-redux parity caller. The parent `ProcessWorker` starts one
fresh worker per scenario, assigns isolated writable storage, and owns
watchdog, cancellation, process cleanup, and victim/survivor checks. The
standalone worker provides the JSON-lines v1 capability, state, input, timing,
RGBA capture, and explicit unsupported-operation surface; it does not duplicate
the parent supervisor.

The acceptance covers five fresh serial and four concurrent sessions, 30 parity
tests, 650 DirPlayer library tests, four native probe tests, and one wire test.
The compact machine manifests retain the source identities, per-file hashes,
commands/results, toolchain, campaign observations, 30 capture hashes, wire
audit, failed raw-input-shape attempt, and executable overwrite/restoration
caveat:

- [`receipt.json`](receipt.json)
- [`campaign.json`](campaign.json)

The worker source digest is `f39ff8b081741972fc4580e2402d6ae48fc46785cb6765faceffc02fa9914aac`;
the six-file caller identity is `c43120cbd4ddb42b5dc2d7e358a6f448ba2a6bfd31256653efd8f092ef9f785a`.
The accepted executable is `eee624fc19d4a9a67a20f1de6e48d4d470c109c3095d2539f0d0e313ae7d1bb6`,
and the fixture is `88b08a92b01e66dddd18b75be665dfaf0bb3b1de6967987d42a8fd082b1b516d`.

Evidence remains under these unambiguous repo-local roots:

- `/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n2-20260919/evidence`
- `/Users/clliaw/Projects/childhood-redux/.cache/parity/native-dirplayer-n2-20260919/evidence`

The results use tested working-tree bytes pinned by the source manifests above.
The accompanying source and manifests identify the tested bytes without a
self-referential publication commit hash.

To build the qualified worker with repo-local writable directories:

```sh
cd /Users/clliaw/Projects/dirplayer-rs
export N2_TARGET=.cache/native-bevy/native-n1-20260918/target
export N2_TMP=.cache/native-bevy/native-n2-20260919/tmp
mkdir -p "$N2_TARGET" "$N2_TMP"
TMPDIR="$PWD/$N2_TMP" CARGO_TARGET_DIR="$PWD/$N2_TARGET" env -u NATIVE_LIFECYCLE_EVIDENCE_DIR mise exec -- cargo build --manifest-path vm-rust/Cargo.toml --bin native_dirplayer_parity_worker --locked --offline
```

The caller single-run command is:

```sh
cd /Users/clliaw/Projects/childhood-redux
export N2_EVIDENCE=.cache/parity/native-dirplayer-n2-20260919/evidence
export N2_TMP=.cache/parity/native-dirplayer-n2-20260919/tmp
export PARITY_DIRPLAYER_RS_ROOT=/Users/clliaw/Projects/dirplayer-rs
export PARITY_NATIVE_DIRPLAYER_WORKER=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/native-n1-20260918/target/debug/native_dirplayer_parity_worker
mkdir -p "$N2_EVIDENCE" "$N2_TMP"
PARITY_SESSION_ROOT="$PWD/$N2_EVIDENCE/single-parent" TMPDIR="$PWD/$N2_TMP" mise exec -- cargo run -p parity --example native_dirplayer_n2 --locked --offline
```

The five-serial/four-concurrent campaign uses the same exported variables:

```sh
PARITY_SESSION_ROOT="$PWD/.cache/parity/native-dirplayer-n2-20260919/campaign-parent" TMPDIR="$PWD/$N2_TMP" mise exec -- cargo run -p parity --example native_dirplayer_n2_campaign --locked --offline
```

Its fixed output directory refuses to run when nonempty. Preserve the existing
`.cache/parity/native-dirplayer-n2-20260919/evidence/campaign` directory or move
it aside before rerunning. Reruns must not promote a baseline automatically.

The caller's source label is metadata. Verification uses the explicit six-file
caller manifest and ten-file worker manifest in `receipt.json`; changing the
source path, build path, toolchain, or source bytes requires an explicit
artifact-pin review. Other local single DCR inputs must pass worker guards but
are not N2 fidelity evidence.

N2 does not qualify PCM, Flash, JavaScript Lingo, external casts, reset,
mutation, invocation, timezone/date behavior, in-process multi-session
ownership, Spybot, Battalion, browser retirement, or N3. N3–N5 remain future work with the
existing provisional, unmeasured 40–122 active-hour Astra estimate.
