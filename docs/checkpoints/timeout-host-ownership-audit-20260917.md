# Timeout host ownership follow-up

Read-only audit after the accepted evaluator-cancellation component. No timer
repair or runtime failure reproduction is claimed here.

`Timeout::schedule` and `Timeout::cancel` dispatch only a name and period through
`JsApi`. `dirplayer-js-api` forwards these notifications to ambient `vmCallbacks`.
In `src/vm/callbacks.ts`, the interval closure captures that registration's root
`browserHandle`, and handles are stored globally in Redux by timeout name alone.
The explicit datum handler still invokes `Timeout::schedule` while operating on
the caller's player. Its checked datum access does not qualify this host effect.

Consequently, the current transport lacks the information needed to distinguish
two players scheduling the same name, and its selected callback registration can
belong to another root. Cancelling one name also lacks an owner discriminator.
This is a source-level ownership gap for Stage 2.3/2.4/2.9, separate from the
accepted retained-timeout datum and evaluator cancellation work.

A bounded repair must qualify schedule/clear/fire with the captured owner and
timer incarnation, retain interval handles in an owner-local host registration,
and perform host work after the mutable VM borrow ends. Reset, disposal and
same-name replacement must retire only their captured interval. Preserve dormant
period-zero behavior, exact-case keys and the existing case-insensitive lookup
fallback; replacing these semantics would not satisfy ownership migration.

Required evidence: two exported browser handles scheduling identical names,
independent firing/clear, replacement with a delayed old tick, reset/disposal,
period-zero activation and callback reentry. Include actual child timer delivery
once child host lifecycle exists. Native behavior requires its own explicit
scheduler/unsupported-effect accounting; the no-op JS stub is not proof.
