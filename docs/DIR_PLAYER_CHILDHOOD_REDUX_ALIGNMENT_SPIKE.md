# DirPlayer / Ruffle / childhood-redux Bevy alignment spike

Status: plan updated; the audit and exactly one bounded experimental Bevy
adoption probe were implemented and executed through the fresh-control gate.
The dependency and focused owner-scoped callback checks passed. After the
probe's nested executor was corrected and `StatesPlugin` was installed before
`init_state`, the fresh normal-source control and Bevy-scheduled path both
completed initialization, t=0 capture, 49 advances, endpoint capture, and
teardown. Both paths matched their 650×420 RGBA captures byte-for-byte and
passed the recorded source-behavior checks. No PCM/audio comparison was made.

## Decision and intended outcome

Pause further game implementation to investigate framework differences before
they produce more piecemeal parity repairs. The user chose Bevy for both projects
to make alignment easier. **The intended direction is to change DirPlayer and its
Ruffle integration to use Bevy directly as much as possible, matching
childhood-redux.**

This is a Bevy adoption spike, not merely a plan to tune independent renderers
until one scene matches. Using the same underlying library is useful interim
progress, but does not by itself establish direct Bevy integration. Likewise,
putting Ruffle's finished image on a Bevy texture establishes presentation
integration, not shared rasterization.

Retain Director/Lingo and Flash/ActionScript execution semantics. Investigate
moving their host services and presentation onto Bevy rather than replacing the
original game logic with childhood-redux logic. Prefer changing DirPlayer/Ruffle
to accommodate childhood-redux's chosen framework. Any necessary change to the
game's architecture must be explained and agreed separately.

The first deliverable is a broad compatibility audit followed by one focused,
executable alignment probe. The audit determined scheduling as the first
boundary for direct Bevy adoption; the probe tested that boundary through its
fresh-control gate before a production migration.

## Evidence motivating the spike

The accepted Spybot menu work exposed differences that were not necessarily bugs
in either implementation:

- Audio decoding and resampling differed between native Ruffle's audio path and
  Bevy's Rodio path. Native Director now uses matching Rodio 0.22.2 primitives.
- Center-pan gain and floating-point mixing order also differed. Matching playback
  admission order was necessary for exact mixed audio, even after decoding matched.
- Source audio bytes and scheduling needed separate qualification: childhood-redux
  now packages the complete original DCR MP3 streams and admits opening audio at
  the source's 50 ms boundary.
- Opening artwork previously exported through FFDec differed from native Ruffle
  rasterization. The accepted recipe generates artwork using pinned native Ruffle;
  this is not evidence that Ruffle and Bevy have equivalent vector renderers.

The existing normal-source menu evidence remains valuable: five serial matched
pairs and four concurrent matched pairs passed 11 visual checkpoints and three
cumulative PCM comparisons through START. Eight runner processes were observed
live together; that observation does not establish simultaneous worker readiness.
Destination visuals, physical-display fidelity, and in-process multi-session
behavior were not qualified by that campaign.

References:

- [Current menu spike and source contract](SPYBOT_MENU_TESTER_SPIKE.md).
- [Menu campaign evidence and reproduction recipe](../../childhood-redux/docs/checkpoints/spybot-native-menu-campaign-20260921/repeated-concurrent-liveness-final/README.md).
- [Native Director audio backend](../vm-rust/src/player/native_audio.rs).
- [Native Flash integration](../vm-rust/src/native_flash.rs).
- [Production Bevy parity worker](../../childhood-redux/workspace/spybot/src/parity_worker.rs).

## Constraints and priorities

1. Adopt Bevy directly wherever it can serve the original runtime faithfully and
   maintainably. Treat a retained separate backend as an exception needing evidence,
   rather than the default architecture.
2. Preserve independently executed source behavior, deterministic controlled time,
   input semantics, session ownership, and clean shutdown.
3. Preserve exact parity requirements. Do not hide differences with sample shifts,
   image normalization, tolerances, substituted outputs, or injected game state.
4. Keep original asset provenance and the existing accepted menu baseline. A shared
   implementation can make two outputs agree while introducing a common defect;
   retain source-level checks and independent evidence where this matters.
5. Prefer a small reusable runtime boundary over Spybot-specific exceptions. Do not
   turn the spike into a general engine rewrite or implement a reusable subsystem
   merely to make a probe pass.

Browser retirement is part of the intended native migration direction. The spike
should identify browser dependencies and their Bevy replacements, but does not
authorize deleting the browser frontend. No further gameplay implementation,
destination screens, full cross-platform certification, or broad ownership
refactor belongs in this spike.

## Audit: identify where Bevy should take ownership

For each area, inspect the actual childhood-redux production path, native
DirPlayer path, and embedded Ruffle path. Distinguish observed differences from
suspected risks and missing capabilities. Record the current owner, proposed Bevy
owner/API, preserved source contract, integration cost, and representative check.

| Area | Questions and representative differences |
| --- | --- |
| Application and scheduling | Can Bevy own the native application lifecycle and host scheduling while the emulators retain source-controlled clocks? How do updates, deferred commands, pause, stepping, and shutdown interact? |
| Rendering and compositing | Which Director drawing and Ruffle rendering operations can become Bevy render work? Examine blending, alpha conventions, color spaces, ordering, masks, filters, vector tessellation, antialiasing, and readback. |
| Images and asset loading | Can Bevy's asset pipeline own decoding, texture creation, caching, and loading without changing source lookup or deterministic readiness? Check palettes, JPEG/PNG decoding, filtering, and sampling. |
| Text and fonts | Can Bevy supply layout and rendering while preserving Director/Flash metrics, embedded fonts, fallback, shaping, and glyph placement? Separate unsupported text from rendering differences. |
| Audio | What remains between the current shared Rodio implementation and direct Bevy audio integration? Include Ruffle's embedded audio, channel lifetimes, gain/pan, speed, looping, source order, and deterministic headless capture. |
| Input and coordinates | Can Bevy events/window services feed the emulators without changing hit testing, capture, cancellation, focus, keyboard behavior, or coordinate rounding? Include logical versus physical pixels. |
| Host services and ownership | Map remaining browser dependencies, preferences, loading, callbacks, handles, and teardown to Bevy resources or adapters. Preserve runtime/session boundaries rather than introducing new global state. |

Classify each proposed change as one of:

- Direct Bevy adoption with a narrow source-semantics adapter.
- A Bevy extension or custom render/audio integration required to express source
  behavior.
- A retained Director/Ruffle implementation with a concrete incompatibility or
  disproportionate cost demonstrated by evidence.

The audit should prioritize the next boundary by expected reduction in repeated
parity work, feasibility, maintenance cost, and risk to original behavior. It is
not an exhaustive survey of every Director or Flash feature.

## Focused probe: test one direct integration boundary

The audit selected application/scheduling as the first representative boundary.
The current Ruffle offscreen renderer and Bevy renderer remain distinct even
though menu artwork now matches. The corrected full-DCR native control and
Bevy-scheduled path completed their lifecycles and matched exact RGBA captures;
the source runtime remains behind the explicit scheduling adapter.

The decision-changing question is:

> Can this subsystem move onto Bevy's actual production machinery while preserving
> the source runtime's behavior and deterministic captures, with a smaller lasting
> compatibility burden than retaining two implementations?

Before editing, specify the exact production path, affected files, workload,
baseline, observable outputs, and stop conditions. Use one real source operation
and the smallest targeted cases that expose the relevant framework differences.
For a rendering probe, choose cases from the audit, such as alpha blending,
fractional positioning, filtering, or a source text/vector operation. Do not claim
vector or font compatibility from a pre-rendered bitmap demonstration.

The probe should:

1. Capture the existing behavior and identify the specific difference or unresolved
   integration risk.
2. Implement a bounded experimental Bevy-backed path in DirPlayer/Ruffle. Keep it
   separable from the accepted production path and avoid game-specific output fixes.
3. Drive it through original source execution and the normal controlled lifecycle,
   including initialization, a meaningful update, capture, and teardown.
4. Compare the relevant outputs exactly and check source behavior independently.
   Reuse applicable menu evidence; rerun only checks affected by the experiment.
5. Report which work Bevy actually owns, which services remain separate, and what
   the probe bypasses. Record failures and untested production conditions.

Success is evidence that the chosen boundary can move onto Bevy without weakening
its contract, plus a concrete next production increment. Failure is also useful:
an identified incompatibility, required Bevy extension, source-semantic conflict,
or cost that changes the migration sequence. Neither result authorizes expanding
the probe into the migration itself.

## Effort allowance and reassessment

The user authorized the audit plus exactly one bounded experimental Bevy adoption
probe after this plan's publication. This work block starts at **2026-09-21
17:13:46 UTC**. Report a checkpoint around **2026-09-21 19:13:46 UTC**, or
earlier when the work completes or is blocked. This is a checkpoint rather than a
hard stop; the work may continue if necessary.

The initial adjustable allocation is:

- Audit: 30 minutes.
- Setup/build: 20 minutes.
- Probe implementation: 40 minutes.
- Probe execution: 15 minutes.
- Interpretation/report: 15 minutes.

After the audit, adjust these allocations if the evidence requires it, but do not
silently broaden the authorized scope. Before execution, record the selected
boundary, affected files, workload, baseline, observable outputs, and stop
conditions. At the audit handoff, estimate the remaining probe cost and reserve
time to interpret its result. The dependency check completed in under a minute
from cache, and the focused callback test passed as helper/witness evidence.
After correcting the probe's nested executor and adding the minimal
`StatesPlugin` setup, the authorized run completed the normal full-DCR control
in 1.08 seconds and the Bevy lifecycle in approximately 1.06 seconds; both
completed by 2.14 seconds total and passed the exact scheduling comparison.

Reassess earlier if:

- Direct integration requires changing original runtime behavior or the exact
  parity contract.
- A new subsystem becomes a prerequisite and materially increases effort.
- The probe only moves a finished texture or buffer into Bevy without resolving
  the implementation boundary it was meant to test.
- Sharing machinery removes the independent evidence needed to check correctness.
- Repeated changes produce neither the promised demonstration nor useful new
  evidence, or a substantially cheaper faithful route emerges.

## Route decision and execution handoff

Preferred route: progressively make Bevy the native host and presentation
framework for DirPlayer/Ruffle, preserving source execution behind explicit
adapters. Use the audit and probe to choose the order and necessary exceptions.

The strongest alternative is retaining specialized Ruffle/Director backends behind
a Bevy host while sharing selected primitives. This may reduce migration cost but
retains more sources of output differences. Recommend it only for boundaries where
the evidence justifies an exception to direct adoption; do not silently substitute
it for the user's intended direction.

Return a compact decision record containing the compatibility map, what actually
ran, exact results and failures, proposed Bevy ownership, necessary exceptions,
remaining uncertainty, and the next component-sized production item with its
acceptance check. The corrected full-DCR control and Bevy comparison are now
qualified for this scheduling boundary. The next item remains a scheduling
adapter retaining `RuntimeSession` and source-controlled clocks; do not bundle
input migration. Execute the authorized audit and probe using the existing
subagent-pair-program workflow; routine technical decisions remain within the
agent team.

The current authorization covered the audit and exactly one bounded experimental
Bevy adoption probe after this plan was published; that work is now complete
through the fresh-control gate. It does not authorize production migration,
gameplay implementation, or browser deletion. No further scope may be inferred
from this document alone.

## Checklist

- [x] Record the user's direction: DirPlayer/Ruffle should adopt Bevy directly as
  much as possible; childhood-redux is the framework alignment target.
- [x] Choose a broad compatibility audit followed by one focused probe.
- [x] Record the authorized work block, checkpoint, adjustable allocation, and
  execution boundary; treat the checkpoint as non-binding and do not broaden scope
  silently when reallocating after the audit.
- [x] Audit the actual implementations and prioritize one Bevy adoption boundary.
- [x] Execute exactly one bounded experimental Bevy adoption probe and review its
  source-behavior/output evidence through the fresh-control gate.
- [x] Decide the migration route, exceptions, and next production increment.

## Execution decision record

The accepted source hash and all five qualified cast hashes passed input
validation. `mise exec -- cargo check --locked --offline --bin
native_bevy_schedule_probe` passed with Bevy pinned to `=0.19.0`, default
features disabled, and a locked offline dependency graph. The focused test
`player::session::tests::native_flash_callback_observer_reports_only_successful_current_owner_once`
passed with one test; it is helper/witness evidence only. It manually invokes
the recording helper after a successful dispatch and rejects a replaced-
generation callback without loading a renderer; it does not exercise the real
`pump_native_flash_segment` observer placement.

The probe adapter was corrected to match the native worker executor context.
The worker uses synchronous `NativeParityWorker::start` and `advance` methods
with one per-operation `block_on`; the probe's normal control is now
synchronous with the same per-operation calls and no outer `block_on`. Bevy
systems remain synchronous and use the same per-operation boundary.

The one authorized rerun was configured with the original full DCR and 49
controlled 50 ms increments in the Bevy-owned lifecycle. After the executor
correction and the minimal `StatesPlugin` setup before `init_state`, the fresh
normal-source control completed initialization, t=0 capture, all 49 advances,
endpoint capture, and teardown in 1.08 seconds. The Bevy path completed the
same lifecycle in approximately 1.06 seconds; both completed by 2.14 seconds
total and passed the exact comparison.

The escalated command was:

```text
env SPYBOT_RESOURCE_ROOT=/Users/clliaw/Projects/childhood-redux/resources/spybot \
SPYBOT_MOVIE=spybot-nightfall-incident.dcr \
SPYBOT_DCR_SHA256=ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77 \
SPYBOT_PROBE_OUTPUT=/Users/clliaw/Projects/dirplayer-rs/vm-rust/.cache/native-bevy-schedule-probe-20260921 \
perl -e 'alarm 120; exec @ARGV' -- mise exec -- cargo run --locked --offline --bin native_bevy_schedule_probe
```

It produced `Finished dev profile`, then `Running target/debug/native_bevy_schedule_probe`,
the control and Bevy phase markers below, and exit 0:

```text
probe.begin                             0 ms
control.input_verification.done          5 ms
control.player_construction.done         7 ms
control.movie_load.done                163 ms
control.presentation_config.done        163 ms
control.callback_observer_install.done  163 ms
control.movie_init.begin               163 ms
control.movie_init.done                470 ms
control.capture_t0.done                479 ms
control.advance.step_1                 497 ms
control.advance.step_25                780 ms
control.advance.step_49               1061 ms
control.capture_endpoint.done         1070 ms
control.teardown.done                 1080 ms
control.complete                      1080 ms
bevy.initialize.begin                 1081 ms
bevy.initialize.movie_init.done       1519 ms
bevy.capture_t0.done                  1528 ms
bevy.advance.step_49                  2122 ms
bevy.capture_endpoint.done            2133 ms
bevy.teardown.done                    2141 ms
native Bevy schedule probe passed
```

The earlier sandbox OpenGL error, escalated 4m40s/exit-130 attempt, and the
intermediate exit-101 run with a missing `StateTransition` schedule remain
historical diagnostics. The corrected executor and explicit `StatesPlugin`
setup invalidated those pre-compare blocker conclusions; the final elevated
run reached both full lifecycles and exited 0.

The final receipt records identical 650×420 RGBA hashes at t=0
(`d4c3ff5167d2c17df73779282ce5a88d8359f5ab309e382c24f40f6d7eaacf03`) and
2.45 s (`78f7734afd3c304d2c96daeb1d6155c374bfc01974dafd080a2477f68ac805d3`),
Director frame 6/label `start`, Flash frame 419, asserted frame 371, empty
pending actions, one owner-scoped callback observation at 2.4 s, and successful
owner retirement after teardown on both paths. No PCM or audio equivalence was
claimed. The scheduling priority remains an inferred hypothesis about repeated
parity-work reduction, not an accepted performance result.

The implementation was kept within the approved files: the Bevy probe owns the
player resource and ordered lifecycle states; `RuntimeSession` and
`NativeFramePump` remain source executors; native callback observation is
optional and behavior-neutral when absent; teardown checks owner retirement and
native binding removal. The probe-local flushed phase logging is retained for
the adapter setup correction and changes no production behavior. The next
component-sized item remains scheduling-adapter-only; the full-DCR control and
this scheduling comparison are qualified, while broader boundaries still need
separate evidence. Do not bundle input migration.

Bevy source-behavior and exact-output parity are accepted for this bounded
scheduling probe. The acceptance does not claim audio equivalence or establish
parity for the other six audit boundaries.
