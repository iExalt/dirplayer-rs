# JS consumer and seed-policy audit — 2026-09-17

Audited implementation: `7d9ca56df79e2027cd0d4f93ee67e93d50070776`. This supplements the accepted 536-test checkpoint; no new runtime changes or test executions are claimed. An independent read-only reviewer checked the requirement mapping and both consumer and seed-policy conclusions.

## Requirements and evidence

| Stage 2.8 requirement | Current implementation and accepted evidence |
| --- | --- |
| Explicit runtime/object owners | `RuntimeSession::js_lingo`, `JsRuntimeRegistry`, and opaque `JsObjectHandle`; registry lookup checks player and exact live owner. Session reset/removal clears only matching entries. `js_object_handles_are_owner_bound_when_numeric_ids_collide`, `js_object_player_reset_clears_registry_and_reuses_id_safely`, and clear/stale-owner tests passed. |
| Owned generic get/set/call | Driver and evaluator classify JS object operations into pending requests; command execution invokes explicit session/player/owner functions. Interpreted calls execute outside the session borrow and validate again before result conversion. Accepted real-session, owner-scheduler and browser fixture tests cover the production routes. |
| Reset and reentry | `explicit_object_call_fences_reset_before_converting_late_result`, weak same-runtime reentry and real ScriptRef proxy reentry passed. Nested invocation budget/depth test passed. |
| Receiver and property semantics | Interpreted browser fixture and factory/value tests retain `this`; exact-case and prototype-resolution tests passed. Indexed property assignment writes the updated collection back. |
| Checked allocation | ID exhaustion and collision tests establish failure before mutation; registering the same object reuses its handle. |
| Runtime-owned deterministic RNG | `JsRuntime` owns captured `Rc<Cell<u64>>` RNG state; installed `Math.random` tests cover default sequence, different/same seeds, interleaving, replay and rejected zero seed. See the earlier RNG receipt. |

The navigator inspected registry ownership, conversion/invocation boundaries, session reset/removal, production call sites and named test receipts. Scoped searches of the loader, JS interpreter directory and JS datum handler found no remaining runtime/object/RNG TLS declarations. This is not the full Stage 2.10 repository-wide ambient-state audit.

## Consumer boundary

Independent review compared `handlers/manager.rs` at `6b0a0a3` and `7d9ca56`; it is unchanged. Global builtin `getProp` and `setProp` support their existing collection/script/cast receivers, not opaque callable `JsObjectRef` receivers. Adding the latter would expand legacy semantics, so it is not required to finish the generic `g.x`, assignment and `g.method()` ownership migration. Plain JS data objects still convert to PropLists and retain their existing collection operations.

Legacy static property/value helpers remain. Normal production JS object and string/value paths are intercepted by owned requests; load-time score initializer evaluation cannot already contain a retained JS object. Direct synchronous JS-object property access fails closed. No migration regression was demonstrated for the unsupported global builtin forms. Broader legacy execution-path removal remains Stage 2.2/2.10 work.

## Seed policy and limits

Stage 2.8 owns runtime RNG state and explicit seeded APIs. The plan places independent-session seed matrices in Stage 3.1 and installation of service-provided seed/epoch/timezone before initialization in Stage 4.1. Existing default seed behavior is preserved; those later requirements are not completed here.

The two Stage 2.8 implementation checklist items and their JS-specific exit invariants are accepted on the linked evidence. Full integration still depends on the remaining Stage 2.3 lifecycle work and Stage 2.10 combined cutover. General evaluator future-drop cleanup and retirement of unused completion adapters, and arbitrary cyclic/alias-preserving value transfer, remain separately tracked. This audit does not close Stage 2 as a whole.


Follow-up call-site audit: a scoped `rg` search across all `vm-rust` Rust files found `commands::take_pending_eval_requests`, `commands::submit_pending_eval_completion` and their session counterparts only in definitions, wrapper calls and documentation. They are crate-private, not exported public APIs. The old adapter removes a completion route before `resume_eval` can reject a malformed value, so retaining it would require route/continuation consistency tests. Removing this unused adapter is the preferred next proposal; no runtime removal is claimed here. In contrast, `drive_eval_owned` has real command and value-evaluator callers and still requires a caller-drop cleanup test.
