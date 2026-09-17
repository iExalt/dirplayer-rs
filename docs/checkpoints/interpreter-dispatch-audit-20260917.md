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
