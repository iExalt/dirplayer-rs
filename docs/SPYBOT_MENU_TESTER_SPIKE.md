# Spybot native menu tester spike

Decision date: 2026-09-19.

## Outcome and priority

Deliver a native DirPlayer reference that supports useful automated testing of
the existing Bevy Spybot menu, then achieve exact parity for that bounded menu
scope. Only after this gate passes should Spybot development and the broader
DirPlayer ownership refactor proceed as parallel workstreams.

The user selected complete menu coverage, including deterministic audio, and
exact Bevy menu parity before that split. A reliable reference that merely
reports the existing Bevy differences is an intermediate result, not completion.

This document records the agreed direction and proposes a bounded discovery
item. Writing this plan does not start a runtime experiment or authorize the
full implementation campaign. No experiment has been run for this spike.

## Scope

- Startup/opening and title presentation, including direct-title test entry.
- Menu state observations and deterministic input/time advancement.
- START button feedback, valid activation, and cancellation behavior supported
  by the original source.
- Menu music and cues, including audio attributable to START activation.
- Exact decoded RGBA and PCM comparisons against the production Bevy systems.
- Repeatability, process isolation, failure reporting, and bounded teardown.

The boundary is START activation. Verify that Bevy emits the expected request
once; do not implement its destination screen. The original proceeds to another
menu while Bevy currently remains on the title. That post-activation divergence
is explicitly outside this milestone, not a passing state comparison.

The dependency audit must define the precise last comparable state/frame and
audio interval. If activation audio continues after the source transition,
identify and validate that cue separately without treating destination visuals,
state, or unrelated audio as part of title parity. Do not silently truncate a
required cue or suppress original behavior to obtain equality.

Deferred: the START destination, tower entry, legal gameplay actions, full
campaign coverage, Battalion, browser retirement, broad reset/worker pooling,
in-process concurrent sessions, and completion of the ownership refactor.

## Current evidence

The existing native implementation provides bounded Director presentation,
timing, input, capture, and a process-isolated worker with a childhood-redux
caller for a synthetic Director fixture. The menu dependency audit must pin
and verify the capabilities reused by this spike; synthetic fixture support
does not qualify Spybot or native Flash/audio.

Current source inspection found:

- The worker advertises state inspection, virtual input, controlled time, and
  RGBA capture. Mutation, invocation, reset, and PCM are unsupported; input is
  limited to stage pointer coordinates and left-button-down dispatch.
- The native load guard rejects embedded Flash, external casts, and JavaScript
  Lingo. The actual menu dependency audit must determine which features are
  required and when; a declared resource is not proof that it is needed at title.
- Native Flash host code and related bindings exist as uncommitted work. Their
  presence does not establish compilation, runtime behavior, or acceptance.
  Preserve that work and establish its identity and ownership before reuse.
- The browser title fixture invokes source initialization, enters the title,
  seeks embedded Flash to frame 419, and verifies that playhead before and after
  2.45 seconds of advancement. This direct entry does not prove normal startup.
- Existing Bevy/browser comparisons report exact pixel and audio differences.
  Native execution does not itself fix them.

The [Spybot menu plan](../../childhood-redux/docs/SPYBOT_MENU_PLAN.md) describes
the existing Bevy menu scope. This sibling link assumes the existing
side-by-side checkouts. This spike focuses on menu testing; broader gameplay
and ownership work remain deferred as described above.

## Constraints and route

Reuse the existing one-scenario-per-process worker and parity protocol,
capture formats, comparison tools, and production Bevy test path. Preserve owner
and generation checks at command boundaries. Broader ownership completion is
not a prerequisite unless a concrete menu failure demonstrates otherwise.

General runtime semantics belong in dirplayer-rs. Spybot resource mappings,
scenario setup, observations, and assertions belong in childhood-redux. Keep
the implementations independent: do not feed reference state into Bevy or share
game behavior in a way that defeats the comparison.

Retain the browser reference to cross-check native behavior against the original
scripts and assets. Investigate browser/native discrepancies separately from
Bevy/native discrepancies; matching two implementations alone does not validate
the reference. No tolerance, resynchronization, normalization, or automatic
baseline promotion may hide a difference.

The recommended route is incremental native menu qualification followed by
exact Bevy fidelity repair. Continuing browser-based testing is the strongest
fallback if native integration proves substantially larger than expected; it
remains useful evidence but does not satisfy the chosen native delivery goal.

## Next bounded item: menu dependency audit

Produce a source-backed scenario contract and a concrete first-probe proposal.
Use retained source and evidence first; do not expand this item into a new
subsystem, gameplay reconstruction, or long runtime campaign.

- [ ] Record checkout revisions, relevant dirty-file identities, nested Ruffle
  identity, existing executable pins, and ownership of unfinished Flash work.
- [ ] Trace normal startup, direct title, button feedback, cancellation, and
  activation through original scripts, score, assets, and existing fixtures.
- [ ] Define the comparison checkpoints, semantic state mappings, input sequence,
  seed/time settings, dimensions, audio format, and activation cutoff.
- [ ] Inventory required Flash operations, resource aliases/casts/fonts,
  inspection/mutation/invocation, rendering, input, and Director/Flash audio.
  Classify each as verified, reusable but unverified, missing, or unnecessary.
- [ ] Separate required menu features from incidental initialization and later
  destination/gameplay dependencies using evidence rather than assumptions.
- [ ] Identify the smallest real-Spybot probe that tests the riskiest dependency.
- [ ] Propose an initial effort allowance and reassessment point, accounting
  separately for setup/build, execution, and analysis. No budget is set here;
  estimate this menu scope independently of deferred gameplay work.
- [ ] Deliver the capability matrix, evidence links, unknowns, revised remaining
  effort, and a bounded implementation/probe item with explicit acceptance.

## Proposed first runtime probe

**Question:** Can the existing native Director path and reusable Flash work run
actual Spybot startup into the title with controlled execution and a completed,
observable Flash render, without requiring the broad ownership refactor?

**Workload:** the original local Spybot DCR and demonstrated required resources,
one worker process, fixed seed and time, normal startup through title, followed
by a separately identified direct-title diagnostic using the existing frame-419
fixture. Setup shortcuts must be recorded and cannot stand in for normal startup.

**Success observations:** source-backed title state; expected embedded Flash
playhead; authored title pixels in a completed Director capture; bounded
advancement; repeatable state/RGBA; clean teardown. Retain source/tool identities,
commands, state, captures, and browser comparison evidence.

**Failure observations:** the first concrete unsupported capability, incorrect
state/render, nondeterminism, resource failure, or lifecycle failure, with a
minimal reproducer. Identifying that blocker is a valid spike result, not menu
acceptance. Do not remove capability guards without implementing and checking
the required behavior, or grow the probe into a reusable subsystem to make it pass.

**Reassessment:** at the agreed effort boundary or when a new major dependency
appears, decide whether to implement the demonstrated gap, revise the native
route, or retain browser testing while reconsidering the cost. The audit must
set that allowance before execution. This first visual probe does not qualify
audio or finish the milestone.

## Delivery sequence after discovery

1. Qualify actual native startup/title execution and deterministic Flash/Director
   composition, including the required state and receiver operations.
2. Qualify menu pointer movement, hover, press/release, cancellation, and START
   activation from original behavior.
3. Qualify deterministic menu audio: source timing, decoding, rate/gain/loop
   semantics, required Flash audio, mixing, and PCM capture. Use isolated cue
   comparisons to find the first divergence before testing the complete mix.
4. Integrate native scenarios into childhood-redux and compare production Bevy
   state, RGBA, and PCM. Correct demonstrated defects in the responsible runtime
   or Bevy implementation, preserving independent reference evidence.
5. Run the final menu campaign and record a durable cross-repository receipt.

These are capability gates, not blanket authorization or detailed estimates.
Keep each implementation item bounded by the next observable result.

## Acceptance and parallel-work handoff

- [ ] Normal startup and direct-title scenarios pass distinct source-backed
  assertions; frame-419 direct entry is not substituted for startup coverage.
- [ ] Declared menu semantic state agrees; exact decoded RGBA and PCM agree at
  every in-scope checkpoint/interval, without hidden alignment or tolerances.
- [ ] Original button feedback and valid/cancelled input sequences are covered;
  Bevy emits one START request for valid activation and none for cancellation.
- [ ] The activation cutoff and separately tested cue tail are explicit; the
  destination remains excluded from any equality claim.
- [ ] Native reference validation retains browser comparisons and script/asset
  evidence, with material discrepancies resolved before declaring the reference
  trustworthy for the chosen scenarios.
- [ ] Five fresh serial and four concurrent runs per declared scenario establish
  exact within-backend repeatability and successful cross-backend comparison.
- [ ] Watchdog, error/cancellation cleanup, process/storage isolation, and
  survivor behavior remain valid for the added native services.
- [ ] Record cold/warm execution costs and retained performance requirements;
  any proposed change to an existing gate is explicit, not a silent relaxation.
- [ ] Runnable commands, pinned source/tool identities, captures, comparisons,
  limitations, and final results are retained in a durable receipt.

After acceptance, pin the qualified tester so Spybot work can continue against a
stable reference while ownership changes are developed and checked separately.
Both workstreams reuse this campaign as a regression gate. Their launch and
detailed scope are later work; this document does not dispatch them.

Revisit the route if a central assumption fails, a substantial rendering/audio
subsystem becomes necessary, ownership changes prove essential to single-process
correctness, remaining effort increases materially, or increments repeatedly
produce neither the promised demonstration nor useful new evidence. Preserve
completed work and exact acceptance criteria when replanning.
