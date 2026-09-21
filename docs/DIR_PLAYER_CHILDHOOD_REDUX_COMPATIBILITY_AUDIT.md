# DirPlayer / Ruffle / childhood-redux compatibility audit and action plan

## Action plan: production headless Bevy scheduling

Decision recorded 2026-09-21 using the agentic-workflow skill. The user selected
**a production headless Bevy scheduling adapter, preserving existing menu RGBA,
PCM, and source behavior**, as the first production deliverable. Planning is
authorized; production implementation and further experiments have not started
under this request. The completed audit/probe record below remains evidence for
the route, not acceptance of the production adapter.

### Outcome, constraints, and route

The existing `native_dirplayer_parity_worker` should run its normal request
protocol through a Bevy-owned lifecycle. Users can run the existing Spybot menu
campaign against that worker without changing its requests, expected source
behavior, or exact output criteria. Bevy must own initialization, ordered
controlled advancement, capture/observation, and retirement as separate systems
or explicit schedules. Director/Lingo and embedded Ruffle/ActionScript retain
source execution behind the adapter.

Hard constraints:

- Preserve exact RGBA and PCM, input ordering, caller-controlled integer time,
  source callbacks, asset provenance, session ownership, and clean shutdown.
- Preserve the worker's protocol, supported operations, structured errors, and
  explicit unsupported operations. Scheduling adoption adds no new input forms.
- Retain `RuntimeSession`, `NativeFramePump`, generation fences, and owner-local
  resources. A Bevy application update must not advance source time implicitly.
- Keep the original source independent of childhood-redux game logic. Do not
  inject state, substitute expected outputs, normalize images, shift samples,
  or weaken equality to make the migration pass.
- Preserve browser build boundaries and current source backends. Browser
  retirement remains a later migration decision; deletion is outside this item.

Ranked priorities are faithful direct Bevy adoption, a useful production worker,
and reduced duplicated lifecycle policy. Sharing renderer/audio primitives alone
does not satisfy the adoption goal. Reduced future parity work is a hypothesis,
not a measured benefit of the completed probe.

Recommended route: promote the demonstrated scheduling boundary into the normal
worker, then qualify it against the existing full menu contract. The strongest
alternative is to keep the existing worker scheduler and share selected host
primitives behind a Bevy frontend. That costs less initially but retains the
duplicated scheduling policy this item is intended to remove. Use that alternative
only if production evidence exposes a concrete incompatibility or disproportionate
cost, and return the outcome tradeoff to the user.

### Evidence and the remaining decision

| Evidence | What it establishes | What it leaves open |
| --- | --- | --- |
| Completed full-DCR scheduling probe; local `receipt.json` inspected during planning | Separate Bevy lifecycle systems, 49 controlled 50 ms advances, exact 650×420 RGBA at t=0 and 2.45 s, one owner-scoped callback at 2.4 s, expected source state, teardown | PCM; arbitrary request sequences; worker protocol integration; failure cleanup |
| Existing [menu campaign and recipe](../../childhood-redux/docs/checkpoints/spybot-native-menu-campaign-20260921/repeated-concurrent-liveness-final/README.md) | Recorded normal-source menu coverage: 11 visual checkpoints, three cumulative PCM comparisons, serial/concurrent isolated processes | Bevy-scheduled production worker; in-process multi-session behavior; destination gameplay; physical display |
| Current worker/probe source inspection | Compatible per-operation executor boundary; native-only Bevy dependencies already present in the working tree | Maintainable production extraction and unchanged admission/capture ordering |

No runtime was rerun for this planning update. The scheduling probe is sufficient
to choose the next item; do not repeat its discovery phase as a prerequisite.
The cheapest next decision-changing check is a fresh pre-change worker baseline
followed by one matched menu run through the candidate adapter, comparing all
three PCM captures as well as RGBA and source observations. Run that early after
integration, before expanding regression coverage. A PCM failure means scheduling
or admission ordering is unresolved; it does not authorize replacing the audio
backend.

Dependencies are classified as follows:

- **Demonstrated prerequisites:** original DCR and qualified casts; the actual
  embedded `ruffle/` gitlink; current worker and childhood-redux campaign harness;
  native GPU execution for captures; explicit owner/executor boundaries.
- **Chosen design constraints:** native-only Bevy, normal worker protocol, exact
  fidelity, no source/game-logic changes, and macOS-first evidence.
- **Untested assumptions:** the probe's lifecycle can serve every existing worker
  request without changing audio admission or error behavior; the extraction
  reduces lasting duplication. These are acceptance questions for this item.

### Next item S1: integrate the production scheduling adapter

One component-sized item, owned by the DirPlayer native host implementation.
Childhood-redux supplies the existing acceptance harness; its game architecture
and embedded Ruffle source are outside the implementation surface.

Expected paths are `vm-rust/src/native_parity_worker.rs`, a focused native host
adapter module, `vm-rust/src/lib.rs`, and focused lifecycle/protocol tests.
Reuse the current native-only dependency setup. Touch `Cargo.toml`/`Cargo.lock`,
`player/session.rs`, or `player/testing.rs` only where the adapter or necessary
observations require it. The probe is reusable evidence and an implementation
reference; its fixed 49-step script is not the production scheduling contract.

Implementation checklist, in dependency order:

- [ ] **S1.0 — Establish a reproducible starting point.** Inventory the existing
  dirty probe/source changes, distinguish them from new adapter work, and retain
  their provenance. Record DirPlayer, childhood-redux, and embedded Ruffle
  revisions, dependency lock state, worker identity, and fixture hashes. Capture
  the pre-migration worker's normal menu outputs before changing its scheduler.
  The campaign runner currently rejects tracked DirPlayer changes
  (`tracked_clean_except(&dirplayer, &[])`); arrange a reviewed, scoped source
  checkpoint before its acceptance runs. Do not disable that guard, stage all
  WIP, discard changes, or silently create a worktree to pass it. If unresolved
  work prevents that checkpoint, report the specific blocked acceptance step.
- [ ] **S1.1 — Extract explicit lifecycle ownership.** Give the Bevy app a
  session-owned non-send player resource and ordered initialization, advancement,
  observation/capture, and teardown systems. Keep synchronous systems with the
  existing per-operation `block_on` boundary; install required state support
  before state initialization. Preserve source executors and generation checks.
  Bevy must not merely call the old worker loop from one opaque system.
- [ ] **S1.2 — Connect the normal worker protocol.** Keep transport and wire
  validation at the boundary; route accepted operations to the host in request
  order. Preserve inspection, invocation, existing stage-space pointer input,
  RGBA/PCM capture, and shutdown behavior. Honor arbitrary currently accepted
  advance durations, including zero, the 60-second limit, and overflow rejection.
  App bookkeeping, inspection, input, and capture must not introduce extra source
  ticks. Preserve the existing policy for errors during advancement.
- [ ] **S1.3 — Run the early fidelity check.** Compare a fresh normal-source menu
  run against the pre-change control and unchanged childhood-redux worker. Require
  the 11 exact RGBA checkpoints, three exact cumulative PCM comparisons, and
  independent source observations. Record the earliest differing operation if it
  fails. Investigate that boundary before adding more migration work.
- [ ] **S1.4 — Qualify lifecycle and protocol edges.** Check pre-start requests,
  repeated start, invalid duration, unsupported operations, failed initialization,
  runtime errors, normal shutdown, and EOF/transport-error cleanup. Check exact-once
  callback delivery, stale-generation rejection, and owner/presentation/Flash
  retirement. Add focused tests for real ordering and cleanup risks, using existing
  fixtures; do not create an unrelated runtime/session redesign.
- [ ] **S1.5 — Accept and integrate.** Make the Bevy host the normal worker path,
  remove migration-only duplicated lifecycle policy, and run the final campaign
  against that exact candidate. Record what Bevy owns, retained backends, command
  recipes, exit statuses, revisions, outputs, and remaining limitations. A build,
  the old probe, or an optional adapter path alone cannot close S1.

### S1 acceptance and verification sequence

1. Build the candidate using the pinned tools, initially with
   `mise exec -- cargo build --manifest-path vm-rust/Cargo.toml --locked --offline --bin native_dirplayer_parity_worker`
   from the DirPlayer root. Run the focused lifecycle/protocol tests selected by
   the actual extraction. Check the WASM library build boundary if shared module
   declarations or dependency configuration change; an unavailable target is a
   reported verification gap, not evidence of compatibility.
2. Run the early paired menu check with unique receipt/raw directories. Compare
   candidate and pre-change outputs at identical inputs and times; retain the
   historical accepted anchors and independent childhood-redux comparison so
   shared machinery cannot silently redefine correctness. PCM equality includes
   sample count, rate, channels, and exact sample bytes at the same positions.
3. Re-establish the probe's source witnesses in the production host: asserted
   Flash frame 371 at initialization; frame 419, Director frame 6/`start`, empty
   pending actions, and exactly one successful current-owner title callback at
   the qualified endpoint. Retain the menu campaign's START/cancel observations.
   Validate cleanup after both success and the focused failure paths.
4. Once those checks pass, use the linked campaign recipe with newly built worker
   paths and `SPYBOT_MENU_SERIAL_MODE=fresh`: five serial matched pairs and four
   concurrent pairs, exact visual/audio equality, successful shutdown/reaping,
   and the existing eight-process liveness observation. Record actual revisions
   and worker identities. Run GPU/elevated or network-dependent commands outside
   the sandbox through the approved execution route. Reuse the harness; no new
   general-purpose testing framework is required.

S1 is accepted only when the normal production worker uses Bevy lifecycle
scheduling and every applicable gate above passes. Isolated-process concurrency
does not qualify in-process multi-session concurrency or simultaneous readiness.
Menu acceptance does not qualify destination gameplay or physical-display fidelity.

### Effort and reassessment

Planning estimate for S1 is **4–8 focused engineering hours**, with uncertainty
in worker extraction and PCM ordering. This is a proposed allocation, not elapsed
time, a guarantee, or execution authorization:

| Work | Initial allowance |
| --- | --- |
| Preserve/reconcile WIP, build baseline, record reproducible inputs | 0.5–1 h |
| Extract host lifecycle and integrate protocol | 1.5–3 h |
| Early RGBA/PCM/source comparison and bounded diagnosis | 0.5–1 h |
| Error/cleanup checks and final campaign | 1–2 h |
| Interpret evidence and finish the handoff | 0.5–1 h |

At approximately two hours of a future execution block, report what the normal
worker can now run, whether the first PCM comparison has been reached, and the
remaining estimate. If integration has not reached that comparison, identify the
specific obstacle before investing in additional architecture.

Reconsider sooner if source semantics or exact parity would need to change,
another subsystem becomes a prerequisite, the adapter becomes an opaque wrapper,
independent correctness evidence disappears, or repeated repairs yield neither
a working comparison nor new discriminating evidence. If remaining work materially
exceeds this range, explain the revised cost and route choices. Do not silently
expand S1 into audio/render migration or relax its acceptance criteria.

### Later milestones: dependency-gated, not authorized by this plan

| Order | Useful outcome | Entry condition and exit evidence |
| --- | --- | --- |
| S2 — Input/coordinates | Bevy window/input services feed source adapters faithfully | After S1; test Retina/logical coordinates, letterboxing, edge rounding, press origin, capture/cancel, focus loss, and source hit tests. Qualify additional input forms explicitly. |
| S3 — Host services/ownership | Bevy owns host-facing services without weakening sessions | After S1; qualify two owner-scoped lifecycles, stale callbacks, service unbinding, and teardown. In-process concurrency needs its own evidence. |
| S4 — Images/assets | Bevy manages host assets/readiness while source lookup remains faithful | After lifecycle/service boundaries settle; qualify paths/hashes, casts, palettes, exact decoded pixels, and deterministic readiness. |
| S5 — Audio ownership | A Bevy integration expresses Director/Ruffle audio semantics | Preserve S1 audio evidence; first probe admission order, channels, looping, pan/gain/rate, and fixed-position PCM. Shared Rodio is insufficient. |
| S6 — Rendering/compositing | Bevy owns actual source drawing operations | First select an operation-level workload for inks/alpha, ordering, masks, transforms, filters, or vectors; require exact RGBA and independent source state. Finished-image upload is not this milestone. |
| S7 — Text/fonts | Source text metrics and glyph rendering are qualified | Establish the missing cast-font path and a metric oracle; compare embedded/fallback metrics, positions, and exact RGBA before adoption. |

These are sequencing priorities, not a requirement to implement every earlier
subsystem before investigating a later one. Detail only the next selected item
using the evidence available then. Retained Director/Ruffle rendering and fonts
are evidence-backed exceptions today; revisit them when an operation-level check
can change the decision. Do not replace the intended adoption direction with a
permanent wrapper by default.

Gameplay remains paused under the alignment direction. A usable desktop release,
browser retirement, broader platform certification, and gameplay resumption need
their own outcome and acceptance decisions; S1 is not a proxy for any of them.

### Decision record and handoff

- **Chosen:** production headless Bevy scheduling first; exact menu RGBA, PCM, and
  original source behavior remain mandatory.
- **Proven:** bounded Bevy scheduling feasibility recorded by the completed probe.
- **Unproven:** production protocol/lifecycle integration and PCM preservation
  under that integration; all later migration milestones.
- **Next action:** S1.0, followed by one scheduling-adapter implementation item
  with an early full-menu comparison. No second exploratory subsystem probe is
  needed before starting this route.
- **Authority:** this request produces the plan only. When implementation is
  requested, hand S1 to `subagent-pair-program`; routine implementation, review,
  and verification stay within that bounded item. Later milestones remain backlog.

## Completed audit and probe record

Status: source audit complete; one bounded scheduling probe implemented and
executed through its fresh-control gate. After correcting a probe-introduced
nested executor and installing `StatesPlugin` before `init_state`, the fresh
normal-source control and Bevy-scheduled path completed initialization, captures,
49 advances, and teardown. Both paths matched exact 650×420 RGBA captures and
the recorded source-behavior checks. No PCM/audio comparison was made. This
completed record covers the approved alignment spike only. It does not authorize
production migration, gameplay implementation, browser deletion, or a second
probe.

## Scope and evidence rules

The audit compares the source currently checked out at:

| Repository | Revision | Working-tree condition |
| --- | --- | --- |
| `childhood-redux` | `main` `f041c1ef6ecc994545383998d1294a0b8dea4aee` | unrelated untracked artifacts present |
| `dirplayer-rs` | `dev` `984b86d7b223981f3dd80bc3d4b5b7de14082ac9` | unrelated untracked `.cache/` present |
| `dirplayer-rs/ruffle` (embedded gitlink) | detached `fd5d8dda1cc7b8cf91de141a48574750fa8f86b2` | clean detached submodule; this is the Ruffle used by `vm-rust` |

The actual embedded Ruffle dependency is the `dirplayer-rs/ruffle` gitlink at
`fd5d8dda1cc7b8cf91de141a48574750fa8f86b2`; `vm-rust/Cargo.toml` resolves
`ruffle_core` and `ruffle_render_wgpu` through `../ruffle/...`
([`vm-rust/Cargo.toml:77-78`](../vm-rust/Cargo.toml#L77-L78)). The sibling
checkout `/Users/clliaw/Projects/ruffle` at
`2dfcfedb612509e5d31e7b6e710194b8d3e5e208` is tracked-clean supplemental
comparison material only; it is not wired into the current DirPlayer build.
Claims below about embedded core, rendering, audio, or fonts cite the nested
submodule. Any desktop `winit` or `fontdb` reference is explicitly a
standalone-frontend comparison; embedded scheduling is owned by DirPlayer's
`NativeFlashHost` and controlled Ruffle ticking.

The accepted menu receipt was produced at childhood-redux revision
`ea7bb18e...` and dirplayer-rs revision `64fa557...`, rather than the current
audited revisions above. Later DirPlayer source changes include native-audio
admission ordering, and later childhood-redux commits publish repaired or
repeated campaign evidence. The receipt's RGBA hash is therefore a historical
fail-closed anchor only; a fresh same-revision control is required before any
probe conclusion.

Claims use these labels:

- **Observed**: directly supported by the cited source or accepted receipt.
- **Inferred**: a design conclusion from observed source behavior; it requires
  the stated representative check before acceptance.
- **Missing**: the source path or capability was not found, or is explicitly
  unsupported by the current implementation.

Every proposed boundary is classified as one of:

1. **Direct Bevy adoption with a narrow source-semantics adapter**.
2. **Bevy extension/custom integration required to express source behavior**.
3. **Retained Director/Ruffle implementation**, where incompatibility or cost
   currently outweighs the benefit and the exception is evidence-backed.

The accepted normal-source menu campaign remains the parity baseline. It covers
isolated process behavior through START and does not establish in-process
multi-session behavior, physical-display fidelity, or general gameplay.

## Compatibility map

### 1. Application and scheduling

**Observed.** childhood-redux installs source-facing systems into Bevy
`Startup`, `PreUpdate`, `Update`, and state-transition schedules
([`workspace/spybot/src/lib.rs:40-59`](../../childhood-redux/workspace/spybot/src/lib.rs#L40-L59)).
Its headless worker explicitly controls time with `TimeUpdateStrategy` and calls
`App::update()` during readiness and advancement
([`workspace/spybot/src/parity_worker.rs:183-205`](../../childhood-redux/workspace/spybot/src/parity_worker.rs#L183-L205),
[`workspace/spybot/src/parity_worker.rs:548-572`](../../childhood-redux/workspace/spybot/src/parity_worker.rs#L548-L572)).

DirPlayer has a stronger source-runtime ownership boundary: `RuntimeSession`,
`NativeFramePump`, and owner-checked command processing. The native worker still
orchestrates start, initialization, advancement, capture, and shutdown in its
request dispatcher ([`vm-rust/src/native_parity_worker.rs:448-601`](../vm-rust/src/native_parity_worker.rs#L448-L601),
[`vm-rust/src/native_parity_worker.rs:813-845`](../vm-rust/src/native_parity_worker.rs#L813-L845)).
The embedded Ruffle player advances only through DirPlayer's controlled host:
`NativeFlashHost::advance_frames` and `tick_exact` drive the nested Ruffle
player before capture ([`vm-rust/src/native_flash.rs:511-581`](../vm-rust/src/native_flash.rs#L511-L581),
[`ruffle/core/src/player.rs:530-574`](../ruffle/core/src/player.rs#L530-L574)).
The sibling Ruffle desktop `winit` event loop is supplemental standalone
frontend comparison and is not wired into current DirPlayer.

**Proposed owner/API.** Bevy `App` schedules and controlled time should own the
native host lifecycle. `RuntimeSession`, `NativeFramePump`, Director clocks,
Ruffle clocks, owner tokens, deferred commands, and source callbacks remain
behind a narrow adapter. Initialization, ordered fixed-time updates, capture,
observation, and teardown must be separate Bevy systems or explicit schedule
states. The adapter must not hide the existing worker loop inside one opaque
system call.

**Preserved semantics.** Caller-controlled monotonic time; source frame and
timer ordering; owner/generation fencing; callback delivery; deterministic
capture; clean retirement of session resources.

**Status and cost.** The Bevy host machinery is observed in childhood-redux;
the direct DirPlayer integration is missing. This is inferred to be the best
first boundary because it removes duplicated lifecycle/scheduling policy with
limited impact on source execution. It requires a native-only Bevy dependency
and a separable adapter. Classification: **direct Bevy adoption with a narrow
source-semantics adapter**. Representative check: the probe specified below.

### 2. Rendering and compositing

**Observed.** childhood-redux uses Bevy's camera, viewport, sprite, and render
machinery ([`workspace/spybot/src/canvas.rs:12-48`](../../childhood-redux/workspace/spybot/src/canvas.rs#L12-L48)).
Its accepted opening artwork is packaged as image assets, so this is not yet
evidence that Bevy rasterizes Director or Flash drawing operations.

DirPlayer's canonical Director presentation remains a CPU bitmap compositor,
including viewport cropping and owner-bound bitmap caching
([`vm-rust/src/rendering.rs:470-500`](../vm-rust/src/rendering.rs#L470-L500),
[`vm-rust/src/rendering.rs:504-628`](../vm-rust/src/rendering.rs#L504-L628)).
Embedded Ruffle separately constructs an offscreen WGPU renderer, renders a
frame, reads it back, and applies RGBA to the Director instance
([`vm-rust/src/native_flash.rs:338-401`](../vm-rust/src/native_flash.rs#L338-L401),
[`vm-rust/src/native_flash.rs:600-638`](../vm-rust/src/native_flash.rs#L600-L638)).
The embedded Ruffle renderer contract includes shape registration, bitmap
registration, offscreen rendering, filters, frame submission, and readback
([`ruffle/render/src/backend.rs:28-119`](../ruffle/render/src/backend.rs#L28-L119)).

**Proposed owner/API.** Keep Director's CPU compositor and Ruffle's renderer
until operation-level evidence identifies a faithful Bevy render extraction or
custom render pipeline. A future Bevy integration must own actual source
operations such as alpha/ink compositing, masks, filters, vector tessellation,
and ordering; uploading a finished bitmap is insufficient.

**Preserved semantics.** Palette and ink rules, source ordering, masks,
transforms, filters, text placement, alpha conventions, and exact RGBA readback.

**Status and cost.** Direct ownership is missing and high risk. The current
exception is evidence-backed by the separate renderer contracts and the lack of
operation-level Bevy coverage. Classification: **retained Director/Ruffle
implementation**, with a possible future **Bevy extension/custom integration**.
Representative check: a later operation-level probe must compare exact RGBA and
an independent source-state observation; the current probe intentionally does
not claim render migration.

### 3. Images and asset loading

**Observed.** childhood-redux loads image and audio handles through Bevy's
`AssetServer`, then waits for root, dependency, and recursive load states before
entering the opening state ([`workspace/spybot/src/loading.rs:15-39`](../../childhood-redux/workspace/spybot/src/loading.rs#L15-L39),
[`workspace/spybot/src/loading.rs:60-117`](../../childhood-redux/workspace/spybot/src/loading.rs#L60-L117)).

DirPlayer validates resource roots, movie paths, source hashes, and optional
external-cast aliases before constructing `TestPlayer`
([`vm-rust/src/native_parity_worker.rs:520-577`](../vm-rust/src/native_parity_worker.rs#L520-L577)).
Director cast lookup, palette interpretation, and embedded Flash bytes remain
runtime-owned. Ruffle's loader and navigator own SWF movie resources inside its
player.

**Proposed owner/API.** Bevy's asset pipeline may own packaged host assets,
texture creation, and readiness reporting. A source adapter must retain
Director cast lookup, path/hash qualification, palette behavior, embedded
format handling, and deterministic readiness barriers.

**Preserved semantics.** Original source paths and aliases; palette and image
decoding; deterministic load completion; no render or script execution before
required assets are ready.

**Status and cost.** Bevy asset ownership is feasible for host-owned assets but
does not replace source parsers. Classification: **Bevy extension/custom
integration**. Representative check: source-path/hash qualification plus exact
decoded pixel comparison at a controlled readiness boundary.

### 4. Text and fonts

**Observed.** The current childhood-redux menu uses packaged image assets, so it
does not qualify source text layout. DirPlayer has a custom PFR/system-font
pipeline; its font manager explicitly reports that actual cast font loading is
not implemented in one path ([`vm-rust/src/player/font/mod.rs:523-546`](../vm-rust/src/player/font/mod.rs#L523-L546)).
The embedded Ruffle core retains font descriptors and embedded-font lookup
([`ruffle/core/src/font.rs:20-80`](../ruffle/core/src/font.rs#L20-L80),
[`ruffle/core/src/library.rs:278-287`](../ruffle/core/src/library.rs#L278-L287)).
Its desktop `fontdb` setup is only a standalone-frontend comparison
([`ruffle/desktop/src/player.rs:120-140`](../ruffle/desktop/src/player.rs#L120-L140),
[`ruffle/desktop/src/player.rs:359-404`](../ruffle/desktop/src/player.rs#L359-L404));
it is not the embedded DirPlayer font owner.

**Proposed owner/API.** Keep Director/Ruffle font metrics, fallback, shaping,
embedded fonts, glyph placement, and line layout. A Bevy text extension is
credible only after a metric oracle and cast-font loading path exist.

**Preserved semantics.** Advances, baseline and line height, wrapping, fallback,
embedded bitmap/vector font behavior, and source field metrics.

**Status and cost.** Direct Bevy text ownership is unqualified and source risk
is high. Classification: **retained Director/Ruffle implementation**, with a
future **Bevy extension/custom integration** if metrics can be proven.
Representative check: source text measurement and exact glyph-position/RGBA
comparison on an embedded and fallback font fixture.

### 5. Audio

**Observed.** childhood-redux uses Bevy `AudioPlayer` in production, but its
headless parity route disables `AudioPlugin` and manually decodes/resamples
through the same Rodio primitives ([`workspace/spybot/src/headless.rs:48-105`](../../childhood-redux/workspace/spybot/src/headless.rs#L48-L105)).
The worker separately discovers entities, applies playback settings, mixes, and
captures PCM ([`workspace/spybot/src/parity_worker.rs:361-405`](../../childhood-redux/workspace/spybot/src/parity_worker.rs#L361-L405)).

DirPlayer already uses Rodio decoder, gain, speed, channel normalization, pan,
queues, and owner-local channels ([`vm-rust/src/player/native_audio.rs:79-103`](../vm-rust/src/player/native_audio.rs#L79-L103),
[`vm-rust/src/player/native_audio.rs:178-218`](../vm-rust/src/player/native_audio.rs#L178-L218)).
Ruffle's `AudioBackend` has a broader contract for embedded sounds, streams,
transforms, ticking, sample history, and playback state
([`ruffle/core/src/backend/audio.rs:89-197`](../ruffle/core/src/backend/audio.rs#L89-L197)).

**Proposed owner/API.** Bevy resources may own host playback, but a custom
deterministic integration must preserve source admission order, channels,
looping, gain/pan, rate shift, sample positions, and PCM capture. Shared Rodio
primitives do not establish shared scheduling semantics.

**Status and cost.** Decode/resample sharing is observed; direct Bevy ownership
of embedded Ruffle/Director semantics is missing. Classification: **Bevy
extension/custom integration**. Representative check: exact PCM at fixed sample
positions plus independent channel-lifetime and source-admission observations.

### 6. Input and coordinates

**Observed.** childhood-redux's `StageViewport` maps physical window coordinates
to a logical stage and rejects letterbox bars and exclusive right/bottom edges
([`workspace/spybot/src/viewport.rs:7-113`](../../childhood-redux/workspace/spybot/src/viewport.rs#L7-L113)).
The menu uses Bevy button input, focus, capture, and source-defined hit bounds
([`workspace/spybot/src/menu.rs:61-157`](../../childhood-redux/workspace/spybot/src/menu.rs#L61-L157)).

DirPlayer's qualified native worker currently accepts stage-space pointer input
and left-button down/up only ([`vm-rust/src/native_parity_worker.rs:748-810`](../vm-rust/src/native_parity_worker.rs#L748-L810)).
Embedded Flash coordinates are scaled from local sprite dimensions to native
movie dimensions ([`vm-rust/src/native_flash.rs:281-297`](../vm-rust/src/native_flash.rs#L281-L297)).
The sibling Ruffle desktop path maps winit window positions into movie
coordinates and dispatches `PlayerEvent`s ([`ruffle/desktop/src/app.rs:90-158`](../ruffle/desktop/src/app.rs#L90-L158));
this is a standalone-frontend comparison, not the embedded DirPlayer input
owner. Its input manager separately tracks physical keys, logical keys, clicks,
and focus semantics.

**Proposed owner/API.** Bevy window/input events and a shared viewport resource
should feed source adapters. The adapter must preserve logical/physical pixel
conversion, focus, capture/cancellation, key identity, event ordering, and
coordinate rounding.

**Status and cost.** The boundary is feasible and naturally expressed by Bevy,
but source mapping still needs exact checks. Classification: **direct Bevy
adoption with a narrow source-semantics adapter**. Representative check: retina,
letterbox, press-origin, release, focus-loss, and source hit-test observations.

### 7. Host services and ownership

**Observed.** childhood-redux's worker owns a Bevy `App`, resources, and cleanup
([`workspace/spybot/src/parity_worker.rs:171-180`](../../childhood-redux/workspace/spybot/src/parity_worker.rs#L171-L180),
[`workspace/spybot/src/parity_worker.rs:580-606`](../../childhood-redux/workspace/spybot/src/parity_worker.rs#L580-L606)).
DirPlayer's `RuntimeSession` binds presentation and native Flash services weakly
by player ID ([`vm-rust/src/player/session.rs:3125-3168`](../vm-rust/src/player/session.rs#L3125-L3168));
`TestPlayer` unbinds services, disposes presentation, and retires the owner on
drop ([`vm-rust/src/player/testing.rs:1072-1090`](../vm-rust/src/player/testing.rs#L1072-L1090)).
The sibling Ruffle desktop builder owns renderer, navigator, storage, UI,
filesystem, and audio services ([`ruffle/desktop/src/player.rs:272-318`](../ruffle/desktop/src/player.rs#L272-L318));
this is a standalone-frontend comparison. The embedded DirPlayer owner is
`NativeFlashHost` and its `RuntimeSession` bindings.

**Proposed owner/API.** Bevy resources should own host-facing services and
events. `RuntimeSession`, player IDs, owner tokens, generation fences, and
source-runtime teardown remain explicit and session-owned; they must not become
global mutable ECS state.

**Status and cost.** Bevy can host these resources, but cross-runtime ownership
and asynchronous callback lifetimes require adapters. Classification: **Bevy
extension/custom integration**. Representative check: two owner-scoped
lifecycles, stale callback rejection, service unbinding, and clean teardown.

## Priority and route

The recommended order is:

1. Application/scheduling: inferred largest repeated parity-work reduction and
   lowest source-semantic risk if the adapter preserves caller-controlled
   clocks; the probe below tests feasibility rather than proving that long-term
   reduction.
2. Input/coordinates: Bevy already owns the relevant event/window machinery;
   exact mapping can be isolated and tested.
3. Host services/ownership: makes Bevy the native host while retaining explicit
   session boundaries.
4. Images/assets: useful readiness and caching consolidation, but source parsers
   remain necessary.
5. Audio: Rodio primitives are already shared; scheduling and capture semantics
   remain expensive to unify.
6. Rendering/compositing: high potential parity benefit, but current operation
   and source-behavior risk is high.
7. Text/fonts: metrics and embedded-font capability are not yet qualified.

The strongest alternative is a Bevy host retaining specialized Director and
Ruffle backends while sharing selected primitives. That route lowers migration
cost and protects current semantics, but it leaves separate scheduling,
rendering, and input boundaries and therefore preserves more sources of parity
drift. The selected scheduling boundary better advances actual Bevy adoption
because Bevy owns the application lifecycle and source-driving schedule; it is
not a finished-image presentation wrapper.

## Exactly one bounded executable probe

### Boundary and source operation

Probe application/scheduling by making a Bevy 0.19 `App` own the player as a
resource and enforce ordered initialize, controlled update, capture/observation,
and teardown systems or explicit schedule states. Source execution remains in
`RuntimeSession`, `NativeFramePump`, Director, and Ruffle.

The source operation is the recovered `opening_anim` Flash movie's original
frame-419 `lingo:introTitleReady()` callback. The isolated native Flash test
supports the claim that one callback is emitted when the verified embedded SWF
is advanced from Flash frame 371 to 419 at 2,400,000 µs
([`vm-rust/src/native_flash.rs:1091-1111`](../vm-rust/src/native_flash.rs#L1091-L1111)).
That isolated test is supporting evidence only; it is not the normal full-DCR
baseline.

The normal probe uses the accepted full-DCR source fixture and ends at
2,450,000 µs. The accepted receipt is
[`native-menu-campaign-receipt.json`](../../childhood-redux/docs/checkpoints/spybot-native-menu-campaign-20260921/native-menu-campaign-receipt.json).
Its fail-closed anchor is:

- source DCR SHA-256:
  `ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77`;
- Director frame `6`, label `start`;
- Flash frame `419`;
- asserted Flash frame `371` at initialization;
- no pending Flash actions;
- 650×420 RGBA SHA-256:
  `78f7734afd3c304d2c96daeb1d6155c374bfc01974dafd080a2477f68ac805d3`.

The accepted campaign metadata records
`native_flash_title_callback_proof: false`; the normal full-DCR receipt does
not prove a callback count. The isolated SWF test is operation-level supporting
evidence only. The probe therefore requires an owner-scoped callback or
notification observation local to the Bevy run, if that can be added without a
production behavior change. If that observation is not feasible within this
probe, exact-once callback preservation under the Bevy schedule remains
**Missing** evidence and the probe must stop and escalate rather than infer it
from the Director `start` label.

### Proposed affected paths

The probe remains separable from production migration. After approval, the
expected implementation surface is:

- `vm-rust/Cargo.toml`: native-only Bevy 0.19 dependency resolution;
- `vm-rust/Cargo.lock`: locked native dependency resolution;
- `vm-rust/src/native_bevy_schedule_probe.rs` and
  `vm-rust/src/native_bevy_schedule_probe_main.rs`: Bevy resource, schedule states,
  and owner-preserving adapter;
- `vm-rust/src/lib.rs`: minimal native module exposure if required by the binary;
- `vm-rust/src/player/session.rs` and `vm-rust/src/player/testing.rs`: optional,
  native-only callback observation and teardown witness used by the probe. With
  no observer installed, the existing path is unchanged.

The direct `bevy_state = "=0.19.1"` line remains alongside the exact Bevy
`=0.19.0` package pin because removing it makes the derive expansion fail with
`cannot find module or crate bevy_state`; Bevy's feature enables the component
but does not make that crate name available to this probe crate. Both remain
native-only and default-feature disabled.

No childhood-redux or Ruffle runtime files are part of this probe. Bevy is not
currently a `dirplayer-rs` dependency, so dependency resolution, offline cache
availability, and native build feasibility are explicit setup gates.

### Workload and lifecycle

Use the original full DCR title fixture, with its verified external-cast
qualification and 650×420 presentation. Run:

1. Construct the Bevy `App` and player resource.
2. Initialize the full DCR at controlled `t=0` through the existing native
   runtime entrypoint.
3. Capture and inspect at `t=0`.
4. Run 49 ordered 50 ms controlled-time updates through Bevy-owned systems to
   `t=2,450,000 µs`; do not call the existing worker loop as one opaque system.
5. Capture and inspect at `t=2,450,000 µs`.
6. Drain and validate source observations, then run explicit teardown systems,
   `App` cleanup, and player/resource retirement.

The fresh same-revision control must capture both `t=0` and `t=2.45 s` using
the existing native path. The accepted 2.45 s hash above is a fail-closed
anchor, not a substitute for a fresh control at the same revisions and source
inputs.

### Acceptance comparisons

The Bevy-scheduled path must match the fresh control byte-for-byte at both
captures: 650×420 dimensions, exact RGBA bytes, and exact SHA-256. No image
normalization, tolerances, sample shifts, or substituted output are allowed.

Independent source-behavior checks must establish all of the following:

- Director frame `6` and label `start` at the endpoint;
- Flash frame `419` at the endpoint;
- asserted Flash frame `371` at initialization;
- empty pending Flash actions;
- the full-DCR observation reaches the source-driven `start` state;
- the isolated embedded-SWF test supports the operation-level
  `lingo:introTitleReady()` callback claim, while the Bevy run supplies the
  owner-scoped callback/notification observation;
- owner/session remains live through capture and is retired after teardown;
- no stale callback, presentation binding, or native Flash binding survives
  teardown.

The callback claim must remain attributed correctly: the isolated SWF test
supports the callback operation itself, while the normal full-DCR receipt proves
the accepted endpoint state and output. The receipt's `start` label does not
prove callback count; the successful probe-local owner-scoped observation now
supplies that missing count for this scheduling comparison.

### No-trivial-wrapper gate

The probe fails its adoption question if Bevy only invokes the existing worker
loop as one opaque call, or if it only presents a finished bitmap/byte buffer.
Passing requires Bevy to own the player resource and the ordered lifecycle:
initialization state, fixed controlled-time update systems, capture and source
observation systems, and teardown/retirement state. Director and Ruffle remain
source executors behind explicit adapters, so an exact output match alone is
insufficient.

### Budget and stop conditions

Estimated remaining allocation after the audit is setup/dependency resolution
15–20 minutes, implementation 30–40 minutes, build 10–20 minutes, one probe
run 5–10 minutes, and analysis/reporting 10–15 minutes. Dependency/build
pressure may require reallocating time within this approved probe, but cannot
expand the scope or authorize another probe.

Stop immediately on any of these conditions:

- Bevy dependency resolution or native build cannot be made feasible within the
  setup/build allowance;
- Bevy ownership collapses into an opaque worker call or finished-buffer wrapper;
- owner-scoped callback/notification observation is not feasible without a
  production behavior change, leaving exact-once callback preservation missing;
- source state, callback count, owner lifetime, or pending-action observations
  differ;
- either exact RGBA comparison fails;
- teardown leaves a live owner, callback, presentation binding, or Flash binding;
- implementation requires changing Director/Ruffle source behavior or the
  established parity contract.

## Executed probe result

The dependency and focused observation gates passed. These commands used the
approved `mise` path and cached dependencies:

```text
mise exec -- cargo check --locked --offline --bin native_bevy_schedule_probe
Finished `dev` profile ... (exit 0)
mise exec -- cargo test --offline --manifest-path Cargo.toml --lib \
  player::session::tests::native_flash_callback_observer_reports_only_successful_current_owner_once \
  -- --exact --nocapture
test ... ... ... ok; 1 passed; 0 failed
```

The focused test uses an owned session without loading a renderer. It is helper
and witness evidence only: it proves a successful current-generation dispatch
can be recorded once and a replaced-generation callback is rejected, but it
manually invokes `record_successful_native_flash_callback` and does not exercise
the real `pump_native_flash_segment` observer placement. The final full probe
did exercise that production placement and recorded one current-owner callback.
The observer records only after
`dispatch_native_flash_callback_if_current` returns true, and production code
without the optional observer has no extra observation path.

The one authorized probe command was:

```text
SPYBOT_RESOURCE_ROOT=/Users/clliaw/Projects/childhood-redux/resources/spybot \
SPYBOT_MOVIE=spybot-nightfall-incident.dcr \
SPYBOT_DCR_SHA256=ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77 \
SPYBOT_PROBE_OUTPUT=/Users/clliaw/Projects/dirplayer-rs/vm-rust/.cache/native-bevy-schedule-probe-20260921 \
mise exec -- cargo run --locked --offline --bin native_bevy_schedule_probe
```

The input hashes passed. The earlier sandboxed OpenGL error, escalated
4m40s/exit-130 attempt, and intermediate exit-101 run with a missing
`StateTransition` schedule are retained as historical diagnostics; they
preceded the final corrections and did not qualify the existing worker path.
The final elevated command, with `StatesPlugin` added before `init_state`, was:

```text
env SPYBOT_RESOURCE_ROOT=/Users/clliaw/Projects/childhood-redux/resources/spybot \
SPYBOT_MOVIE=spybot-nightfall-incident.dcr \
SPYBOT_DCR_SHA256=ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77 \
SPYBOT_PROBE_OUTPUT=/Users/clliaw/Projects/dirplayer-rs/vm-rust/.cache/native-bevy-schedule-probe-20260921 \
perl -e 'alarm 120; exec @ARGV' -- mise exec -- cargo run --locked --offline --bin native_bevy_schedule_probe
```

It built, entered the probe, completed both lifecycles, and exited 0 with these
phase markers:

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

The corrected control completed in 1.08 seconds; the Bevy lifecycle completed
in approximately 1.06 seconds, with both paths complete by 2.14 seconds total.
The receipt records identical t=0 RGBA SHA-256
`d4c3ff5167d2c17df73779282ce5a88d8359f5ab309e382c24f40f6d7eaacf03` and
endpoint RGBA SHA-256
`78f7734afd3c304d2c96daeb1d6155c374bfc01974dafd080a2477f68ac805d3` for both
paths, with 650×420 dimensions. It also records Director frame 6/label `start`,
Flash frame 419, asserted frame 371, empty pending actions, one owner-scoped
callback observation at `now_us=2400000`, and successful teardown for both
paths. The final result is a passed bounded scheduling comparison; no PCM/audio
equivalence was claimed.

The retained artifacts are under
`vm-rust/.cache/native-bevy-schedule-probe-20260921/`: `control-t0.rgba` and
`bevy-t0.rgba` both hash to
`d4c3ff5167d2c17df73779282ce5a88d8359f5ab309e382c24f40f6d7eaacf03`,
`control-endpoint.rgba` and `bevy-endpoint.rgba` both hash to
`78f7734afd3c304d2c96daeb1d6155c374bfc01974dafd080a2477f68ac805d3`, and
`receipt.json` hashes to
`da87aada0fdb1c1b6c025a133bcc93af9cfbeb019ad0cb73a1582d1d30fade9e`.

The executor correction removed the probe's outer `async_std::task::block_on`
around the synchronous normal control. The native worker uses synchronous
`NativeParityWorker::start`/`advance` entrypoints with one per-operation
`block_on` ([`native_parity_worker.rs:454-608`](../vm-rust/src/native_parity_worker.rs#L454-L608),
[`native_parity_worker.rs:819-833`](../vm-rust/src/native_parity_worker.rs#L819-L833)).
The corrected probe now follows that context: `run_native_control` is
synchronous, while its individual load/init/advance operations retain their
existing `block_on` calls. Bevy systems are synchronous systems with the same
per-operation calls and no outer executor around the control path.

The no-trivial-wrapper gate was implemented and exercised: Bevy owns a non-send
player resource, explicit initialize/capture/advance/capture/teardown states,
and 49 individual 50 ms transitions; the source runtime remains in
`RuntimeSession`/`NativeFramePump`. The probe-local flushed markers and the
minimal `StatesPlugin` setup change no production runtime behavior. The broad
claim that scheduling offers the largest repeated parity-work reduction remains
an inferred hypothesis.

The final changed-interface inventory is `Cargo.toml`/`Cargo.lock`, the explicit
`[[bin]]` entry targeting `src/native_bevy_schedule_probe_main.rs`, the probe
module, `lib.rs`, the behavior-neutral `QualifiedExternalCasts::from_entries`
exposure in `native_parity_worker.rs`, and the optional observer/witness changes
in `player/session.rs` and `player/testing.rs`. The `from_entries` exposure is
needed because the probe is outside the worker test module; it only constructs
the already-qualified cast map and changes no runtime behavior.

The next component-sized item remains scheduling-adapter-only. The full-DCR
control and this scheduling comparison are qualified; broader boundaries still
need separate evidence. The adapter must retain `RuntimeSession`,
source-controlled clocks, and the existing native renderer boundary; do not
bundle input migration.

## Checklist

- [x] Inspect the published alignment spike and all three production paths.
- [x] Record source-backed owners, proposed Bevy owners, preserved semantics,
      evidence status, costs, checks, and classifications for all seven areas.
- [x] Prioritize the first boundary and document the strongest alternative.
- [x] Specify exactly one executable Bevy adoption probe with full-DCR control,
      exact outputs, independent source checks, lifecycle, stop conditions, and
      estimated setup/build/run/analysis time.
- [x] Implement the approved probe.
- [x] Execute and interpret the approved probe through the fresh-control gate.
- [x] Accept Bevy source-behavior and exact-output parity for the bounded
      scheduling probe, with no PCM/audio claim.
- [x] Record the historical nested-executor and missing-state-schedule attempts
      separately from the final passed comparison.
- [x] Select the next production item from the probe evidence.
