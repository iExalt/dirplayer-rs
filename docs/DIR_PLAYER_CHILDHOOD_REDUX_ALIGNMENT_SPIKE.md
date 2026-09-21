# DirPlayer / Ruffle / childhood-redux Bevy alignment spike

Status: plan updated; publication pending. The audit and exactly one bounded
experimental Bevy adoption probe are authorized, but execution has not started.

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
executable alignment probe. The audit determines the best first boundary for
direct Bevy adoption; the probe tests that boundary before a production migration.

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

The audit selects one representative boundary. Rendering/compositing is an initial
candidate, not a predetermined winner: the current Ruffle offscreen renderer and
Bevy renderer remain distinct even though menu artwork now matches.

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
time to interpret its result.

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
acceptance check. Execute the authorized audit and probe using the existing
subagent-pair-program workflow; routine technical decisions remain within the
agent team.

Current authorization covers the audit and exactly one bounded experimental Bevy
adoption probe after this plan is published. The next action is to carry out that
audit and selected probe under the allocation above. It does not authorize
production migration, gameplay implementation, or browser deletion. No further
scope may be inferred from this document alone.

## Checklist

- [x] Record the user's direction: DirPlayer/Ruffle should adopt Bevy directly as
  much as possible; childhood-redux is the framework alignment target.
- [x] Choose a broad compatibility audit followed by one focused probe.
- [x] Record the authorized work block, checkpoint, adjustable allocation, and
  execution boundary; treat the checkpoint as non-binding and do not broaden scope
  silently when reallocating after the audit.
- [ ] Audit the actual implementations and prioritize one Bevy adoption boundary.
- [ ] Execute exactly one bounded experimental Bevy adoption probe and review its
  source-behavior/output evidence.
- [ ] Decide the migration route, exceptions, and next production increment.
