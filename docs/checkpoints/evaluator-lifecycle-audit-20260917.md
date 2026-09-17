# Evaluator lifecycle follow-up — 2026-09-17

Source audit of checkpoint `4e103fdf`; no lifecycle repair or new execution
result is claimed here. Stage 2.3 remains open.

## Live caller-drop boundary

`player::drive_eval_owned` drives both `invoke_value_request_owned` and
`eval_lingo_command_owned`. It suspends on external extension, movie and Flash
requests, or retains evaluator/child-command routes before awaiting a receiver.
It has no cancellation guard. The production `invoke_request_owned` helper also
starts an evaluator, discards its returned EvalId/action locally, and awaits the
receiver without a cancellation guard. Dropping either future can leave a retained
evaluator or child route behind; this source finding needs a production-entrypoint
poll/drop regression before choosing the repair.

The neighboring `invoke_script_callback_owned` has a `CallbackCancellation`
guard. Its drop calls `RuntimeSession::cancel_eval_callback`, which removes exact
EvalId/player/owner routes, cancels associated command tickets, releases child
drivers and removes matching evaluators. That is a candidate cleanup primitive,
not proof that it covers every state of the general evaluator pump. Its use of
`borrow_mut` on drop also requires an explicit reentry/borrow-lifetime check.

The next bounded implementation should exercise a real pending evaluator, drop
its caller future, and prove that retained routes, scopes and tickets retire.
It must preserve a neighboring evaluator and player with overlapping local IDs,
reject late completions, and prove normal completion and successive actions still
work. Cancellation while an external request is detached must be covered, not
just removal from the pending queue. Host resource cleanup and nested descendant
cancellation need to be traced through the actual request producers.

## Unused completion adapter

A repository-wide Rust call-site search finds only definitions, their thin
wrapper calls and comments for `take_pending_eval_requests_for`,
`take_pending_eval_requests` and `submit_pending_eval_completion`.
These are crate-private adapters, not a demonstrated public production route.
The submission adapter removes a pending/inflight route before `resume_eval`
validates the result. Prefer removing this unused boundary during Stage 2.3/2.10
cleanup rather than adding a second completion protocol. Update stale comments
in `session.rs`, `commands.rs` and `eval.rs` with that removal.

This audit does not establish cancellation coverage for all async producers;
that requires the wider Stage 2.3 inventory and production tests.


## Repair design constraints from independent review

Use one exact EvalId/player/owner cancellation guard for the three caller forms:
the shared drive loop, `invoke_request_owned`, and nested script callbacks.
Existing `cancel_eval_callback` is a candidate common cleanup primitive, but the
repair must prove child-driver, action-ticket, inflight route and continuation
cleanup rather than assuming its current callback scope is sufficient.

A contended Drop must neither panic through `borrow_mut` nor silently abandon
cleanup through a failed `try_borrow_mut`. Any deferred cancellation needs a
concrete drain guarantee, including reentrant drop without a subsequent host
event and session disposal. An unspecified spawned retry is not yet an accepted
design. External-load cancellation must identify an individual waiter/request;
owner-wide or same-name cancellation would incorrectly cancel siblings.

Required tests should poll real production futures to Pending before dropping
them, inspect cleanup, attempt late completion, preserve a neighboring evaluator,
and verify fresh work still completes. Normal completion must disarm the guard.
No implementation is accepted until those boundaries are exercised.


## Closed scheduler and shared-load constraints

The command loop can exit on a closed channel while a root session remains live;
its teardown helper is specific to stale nested owners. A mailbox wake alone is
therefore insufficient. The approved staged design gives each exact EvalId a
shared cancellation flag, set synchronously before a Drop attempts to borrow the
session. All evaluator and child-driver dispatch/completion boundaries must reject
that flag before mutation. Deferred physical cleanup uses owner/id records and a
weak session handle, with pump/reset/remove/drop as cleanup boundaries. Retaining
logically cancelled state until a safe cleanup boundary must be distinguished from
retaining runnable work.

External on-demand loads have another independent identity: only the first waiter
notifies the host, and completion currently validates that original request ID
before satisfying the name bucket. Removing the first waiter must not strand a
live same-name sibling. The shared transport attempt must remain eligible to
complete surviving waiters, while the cancelled waiter cannot resume. Once all
waiters cancel, a late old-attempt result must not complete a new bucket with the
same name. Required regressions cover both cases. This is part of the staged
repair review, not yet integrated or verified behavior.
