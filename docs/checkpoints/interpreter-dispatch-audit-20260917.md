# Explicit interpreter dispatch follow-up

Read-only Stage 2.2 review at published `d8f868fd` with later child Flash work
kept separate. This records live consumers and remaining work, not full gate
acceptance.

The production `DriverContinuation` passes an explicit `ExecutionContext` and
validated scope token into `try_execute_opcode_sync`. The remaining `PLAYER_OPT`
reference in `bytecode/handler_manager.rs` belongs to error-history name lookup;
it is not evidence that ordinary opcode execution selects the ambient player.
That diagnostic consumer still belongs in the final ownership audit.

Handler setup in `player/mod.rs` compiles/caches IR and stores `HandlerFrame.ir`.
The current production driver does not consume that field. The discovered
`run_handler_resumable` calls are benchmark paths. Compiled-runner unit tests and
IR cache population therefore cannot establish the plan's production compiled
execution requirement. A separate component must wire the explicit resumable
runner into production and verify result/error/escape behavior and cleanup.
Removing compiled execution from the claimed surface would not satisfy the plan.

`BuiltInHandlerManager::call_handler` is live through session global dispatch,
JS-Lingo builtin calls and cast-member handlers. It accepts an explicit context,
but `SpriteBox`, `PuppetTransition` and `Preload` still use ambient player
accessors at this checkpoint. The active bounded repair migrates these three
branches and adds same-file owner-isolation and rejected-input regressions. It
must preserve argument-count rules, consumed/ignored arguments, numeric defaults
and clamps, label lookup and existing result behavior.

`StopSound` and `CharPosToLoc` also contain ambient access, but invoke audio or
Canvas/font services; they are excluded from this leaf-state repair and remain
part of explicit service/host migration. Flash, external-event and other global
branches remain separately tracked. Existing driver and session callback tests
cover override precedence and error/depth cleanup; they must be retained in the
next combined validation and do not by themselves prove these remaining paths.

## Compiled integration contract discovered in source

`run_handler_resumable` already accepts an exact `ScopeToken`, `DirPlayer` and
`SymbolTable`. It shares the interpreter's operand/local state and returns
`Done`, `Escape`, or `BackJump`. The production driver can integrate this before
ordinary opcode decoding: completed IR must use normal frame completion, an
escape must decode the runner's updated program counter and use the existing
owned pending-action path, and errors must use normal frame failure/unwind.

The IR loop reports a backward jump after input polling or every 4096 backward
jumps. Production integration must account for that batching in cooperative
yield/cancellation and watchdog behavior; treating one IR batch as one original
opcode is not automatically equivalent. Required tests must drive the actual
continuation with IR enabled, including escape to a deferred host action, resume
into IR, child/error cleanup, stale scope/owner, and breakpoint escapes. Existing
standalone arithmetic/benchmark tests alone do not prove this contract.

## Three-branch candidate awaiting execution

`handlers/manager.rs` candidate SHA-256
`8a3cbb96178a85439f67d332bd066633e5cff9a16e8411a7378b45feb915bd4e`
uses the passed context for SpriteBox, PuppetTransition and Preload. Review
confirmed that validation applies only to consumed arguments: SpriteBox's first
five, PuppetTransition's first argument and optional timing/size only in integer
mode, and Preload's selected last-frame argument. Ignored foreign arguments retain
the previous behavior.

Three new native tests exercise selected-player effects and consumed foreign/stale
reference rejection. Positive cases include final sprite geometry, transition
parameters, case-insensitive labels, default last frame and ignored arguments.
The first combined compilation failed in separate child Flash code before an
executable was produced. No passing test or full Stage 2.2 acceptance is claimed
for this candidate yet.

## Accepted three-branch component

The frozen candidate above now passes three focused tests, all **576 native
tests**, and WASM `cargo check --tests`. The navigator executed the isolated
published base plus accepted allocator tests and this manager file, then checked
all 419 source hashes and the live manager hash.
[Portable receipts](explicit-builtins-20260917/README.md) retain the commands,
source and artifact identity, outputs and limits. The three-branch component is
accepted locally. Browser execution is not claimed by this isolated check, and
full Stage 2.2 remains open.

## Current cooperative accounting

A follow-up source search at `3073bf6b` found `total_backjumps` initialized and
incremented in `DriverContinuation`, but no production reader of that field.
The compiled runner's comment about an existing runaway watchdog therefore does
not establish that the current owned driver enforces one. `IrExit::BackJump`
also carries no count, although its runner may have processed 4096 jumps before
returning. The production integration needs an explicit, tested accounting and
yield policy; copying the old comment or incrementing once per batch is not
evidence of equivalent responsiveness or runaway-loop handling. This is a
source finding for the pending compiled-execution component, not an implemented
watchdog or a new acceptance result.

The same production-entry review found no construction of `BreakpointRequest`
in the owned driver and no consumer of `find_breakpoint_for_bytecode`.
`commands.rs` can execute and retain a `PendingAction::Breakpoint`, but that
plumbing alone does not cause a pause. IR breakpoint escapes must be paired with
an actual owned-driver breakpoint/step check and a production continuation test.
The proposal also needs to examine the owner-specific breakpoint instrumentation
cached on `Rc<HandlerDef>` under a numeric breakpoint generation: sharing a
handler between owners must not reuse another owner's breakpoint decisions,
and changing breakpoints while a frame is suspended must be handled explicitly.

## Production integration proposal review

The reviewed production call chain prepares `HandlerFrame.ir` during handler
setup, but `DriverContinuation::turn_impl` still does not consume it. Integration
must enter IR after exact owner/top-scope validation, route escapes through the
existing opcode/action path, and finish or yield through normal driver handling.
An IR escape can land on a breakpoint after the batch's starting PC, so debugger
checks must also run at that escaped PC before executing it.

Owner-qualified shared cache entries alone are insufficient: a suspended frame
retains its own IR reference. Each active frame must refresh its breakpoint
instrumentation when the owner's breakpoint generation changes, without
replacing its live locals, stack or PC. Debugger resume needs a one-shot skip
marker for the exact scope and PC to execute that opcode once without pausing
again. `AfterBreakpoint` and `AfterStep` also need a real transition out of the
current resuming state. Line stepping uses the existing caller-supplied skip
indices; the proposal must not invent source-line inference.

The first approved implementation batch is limited to the compiled runner's
per-run backward-jump accounting and focused tests. Accounting must survive
`Done` and `Escape` as well as the bounded/input-poll jump exit. This does not
enable production IR execution or close the interpreter gate. Driver/cache/
debugger integration remains a subsequent implementation batch, including
settling paused debugger waiters during owner cancellation.

The accounting batch is now implemented and its four named native tests pass.
The navigator verified the live source hash and actual test output; bounded
acceptance is recorded in `compiled-accounting-20260917/`. Production IR and
debugger integration remain open as described above.

Callback integration follow-up: classify_async_object produces SpriteAsync for
SpriteRef receivers, but its default for other unsupported datums remains an
ordinary Object request. The callback repair uses the typed result; it does not
define new semantics for truly unsupported Object calls. Those may still
requeue unchanged and fail to terminate. Track this residual behavior in the
Stage 2.2 dispatch cutover; callback-specific success cannot close that gate.

The real callback fixture observed Void for getVariable supplying the FlashObject
argument to sprite.setCallback, both inline and standalone. Subsequent source
trace attributes this to the browser runner's bridge transport converting
object-valued getter results to null. This supersedes the tentative nested-eval
diagnosis; these receipts do not establish a generic nested-expression defect.
The callback fixture instead obtains a real bound object from callback delivery.
Track object-return support in the Stage 2.5/2.6 bridge follow-up and do not infer
it from callback-specific success.

The later callback fixture also exposed SpriteAsync's shared waits_for_ready
fallback: it uses numeric readiness and invokes SpriteDatumHandlers::call under
with_player, whose callFunction uses the numeric host route. Overlapping owner
sprite IDs make that host lookup ambiguous. The callback component now has an
approved dedicated callFunction path with captured generation and detached host
work, preserving its existing argument-string and String-or-Void contracts.
Sprite setVariable still reaches the shared legacy block. Subsequent discovery
finds that the recognized getVariable builtin already takes a partial classifier
shortcut into BindGet, bypassing attached-script precedence and failing its
missing-member case; its legacy getter body still exists. Both methods remain
open work. Do not extend callback acceptance to their host-borrow, override or
initial-readiness behavior. See sprite-variable-ownership-audit-20260918.md.

Successor acceptance, 2026-09-18: the dedicated sprite get/set component is now
accepted at `sprite-variable-20260918/`. It removes the getter classifier
shortcut, preserves attached-script precedence, owns both host routes and
verifies early retained-object use and replacement rejection in real Ruffle.
The older open-work statements above describe discovery history. Production IR,
debugger integration and unsupported ordinary Object dispatch remain separate
open Stage 2.2 work.
