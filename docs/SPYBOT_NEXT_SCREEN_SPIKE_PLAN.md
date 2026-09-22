# Spybot START → next-screen reference spike

Status: **planned; no spike or destination implementation has run under this
plan.** Decision updated with agentic-workflow on 2026-09-21. This reference
qualification can run in parallel with [DirPlayer save-state Q0](DIR_PLAYER_SAVE_STATE_PLAN.md).
The previous dependency on finishing save-state qualification is removed.

## Outcome and boundary

The eventual playable increment is START → original NEW GAME/menu destination →
one empty slot's hover, press, drag outside, and release. Reproduce source-defined
artwork, layout, input feedback and audio, with exact native DirPlayer RGBA/PCM
and semantic-state comparison. Preserve accepted title behavior and fresh-session
isolation. Slot activation, occupied slots, game save/load, character selection,
world map and gameplay remain deferred. Emulator checkpoints are a separate lane.

The next useful deliverable is a reproducible reference trace and a bounded Bevy
implementation plan with an evidence-based estimate. This planning change does
not start either experiment or implement the port. Existing in-memory preference
initialization is allowed; no cross-launch persistence feature is needed.

Read-only inspection confirms `childhood-redux/workspace/spybot/src/menu.rs`
currently consumes `StartRequested` by incrementing a counter while leaving the
title active. Recovered source previously identified START's `go("menu")` path,
SaveFilesBH (BehaviorScript101), and SAVE SLOT BUTTON (BehaviorScript102). Those
are discovery leads, not proof of their live attachment or cancellation behavior.
The accepted title campaign qualifies the starting point, not this destination.

## Decision-changing question

Can the pinned native DirPlayer reach the actual destination via normal START
input and expose one empty slot's complete non-activating interaction, including
source graphics/audio and observable state, without a new runtime subsystem?
In particular, does release outside cancel, or does the attached source behavior
activate regardless? Do not infer the answer from the title button or a script
name. If source behavior activates, report the scope conflict before designing
an invented cancellation rule or implementing slot activation.

Recommended route: normal source execution on an isolated reference worker,
followed by a narrow port only after the live trace is qualified. The alternative
is a static reconstruction from recovered assets/scripts; it can guide discovery
but cannot establish live timing, attachment, PCM or cancellation parity. Waiting
for direct checkpoints adds an unnecessary dependency for this short entry path.

## N0: proposed 60–90-minute reference qualification

Use 60 minutes as the first reassessment point and 90 minutes as the proposed
hard cap, including setup/builds and analysis. This retains the earlier proposed
allowance; it is not an implementation estimate. At the cap return evidence and
blockers instead of expanding the experiment into runtime or renderer work.

| Work | Allowance | Output |
| --- | --- | --- |
| Isolate inputs, worker and harness; verify the title baseline | 15–25 min | Exact revisions, build/features, asset hashes and reproducible launch/input commands |
| Reach destination and identify the live control | 15–20 min | Normal START trace, source frame/label, sprite/member/behavior bindings and authored hit rectangle |
| Exercise one empty slot and capture its behavior | 20–30 min | Timestamped input, RGBA, PCM and source-state observations at each boundary |
| Assess route and write handoff | 10–15 min | Qualified scope or precise blocker, next component-sized item and revised estimate |

Run from a fresh session with controlled clock/RNG and known empty in-memory
preferences. Play the original opening/title, then perform START hover/press/
release through the existing input protocol. Do not jump to a frame, inject
menu globals, invoke its click handler directly, or use an emulator save state.
Record transition audio and timing, including any outstanding Flash callbacks.

Once the real destination is stable, identify all visible operands needed to
reproduce it: source draw order, asset/member IDs and hashes, bounds/registration,
ink/blend/palette/alpha/text and any animation. Identify the slot's actual attached
behaviors and authored hit bounds. Capture the idle destination, pointer hover,
press inside, held-pointer move outside, release outside, and subsequent controlled
steps sufficient to observe delayed activation. Record source time at every event;
do not assume a single settled frame proves cancellation.

For each checkpoint retain exact RGBA dimensions/bytes/hash, PCM sample format,
rate/channels/counts and capture interval, and relevant source state: frame/label,
slot status, mode, pressed/hover capture and observed transition requests. Preserve
existing cumulative PCM semantics; derive interval comparisons transparently from
recorded absolute positions. Correlate feedback sounds with source events. A
visually unchanged screen alone does not establish unchanged slot or transition
state. Do not add an inside-release activation experiment to this bounded slice.

Repeat the same trace in a fresh worker to check reproducibility and isolation.
Compare semantic state rather than process-local owner IDs. Preserve commands,
logs, source observations, raw captures and compact reviewable comparison receipts;
identify instrumentation and any conditions it bypassed. Put durable reports under
`childhood-redux/docs/checkpoints/spybot-next-screen-<date>/` and large local captures
under that lane's `.cache/artifacts/spybot-next-screen-<date>/`, using distinct run
IDs. Do not overwrite title campaign evidence.

## Acceptance and resulting route

N0 passes when normal START reaches the source destination, live assets/behaviors
are identified, the entire selected interaction is observed with matching repeated
reference RGBA/PCM and semantic state, and the port's required operations are known.
Report cancellation as proven only if release outside leaves the slot and screen
unactivated across the observed follow-up work. Exact Bevy-versus-DirPlayer parity
remains a later implementation gate; two matching reference runs do not prove it.

If navigation, assets, rendering, audio, input delivery or observation fails,
preserve the smallest reproduction and name the missing capability. Routine setup
or narrow instrumentation may fit within N0; a production runtime repair needs a
separate bounded item and estimate. If unexpected activation occurs, report source
semantics and return the scope tradeoff to the user. Do not silently expand into
save-slot activation or weaken exact equality.

After successful N0, propose the first bounded implementation item: make START
enter the verified destination and reproduce its base presentation, then add the
qualified slot interaction as a subsequent item if needed. Estimate from observed
operands and dependencies; keep later work coarse. Final port acceptance requires
identical scripted inputs/times against DirPlayer, exact RGBA/PCM and source-state
checks across transition and interaction, title regressions and fresh-session
isolation. No production implementation estimate is established by this plan.

## Parallel execution and merge contract

Follow the branch/worktree table and integration rules in the
[save-state plan](DIR_PLAYER_SAVE_STATE_PLAN.md#parallel-branches-worktrees-and-integration).
Use local `next-screen` branches in sibling worktrees for both DirPlayer and
childhood-redux; save-state uses local `save-state` worktrees. Pin the reference
worker by absolute path and revision. Resolve the harness's runtime/source paths
explicitly so it cannot pick up the other lane's worker or edits. Isolate builds,
caches and captures; share only immutable verified source assets.

N0 needs neither checkpoint support nor completion of Q0. Q0 needs neither N0 nor
the destination port. Findings may inform each other without changing pinned
baselines. Qualify any necessary shared fix independently before adopting it.
Work may proceed independently while GPU-heavy runs are scheduled to avoid
contention. At execution handoff use subagent-pair-program for one bounded item
at a time, with a separate worktree assignment for each lane.

Commit/push completed milestone evidence on the topic branches; merge accepted
DirPlayer work into `dev` and Spybot work into `main` after review and relevant
checks. Revalidate affected parity gates on the combined tree when the other lane
has merged first. Preserve exact runtime/fixture compatibility; rebuilding with
save-state changes creates a new identity, not permission to reuse incompatible
checkpoint files. A completed spike can merge as evidence while the port remains
unimplemented.

## Checklist

- [x] Preserve the small interaction scope and defer slot activation/persistence.
- [x] Remove the dependency on save-state qualification.
- [x] Specify isolated local branches/worktrees and eventual integration gates.
- [ ] Execute N0 within its bounded allowance and publish evidence or blockers.
- [ ] Select and estimate the first port implementation item from N0 evidence.
- [ ] Implement, compare, and merge accepted increments with title regressions.
