# Evaluator caller cancellation

Accepted bounded component based on `243647a3`, including the accepted
owner-generation repair. This does not close the complete Stage 2.3 async audit.

Dropping an owned evaluator caller synchronously cancels its exact capability.
Cleanup runs immediately when the session is available, or through a weak-session
mailbox and owner pump when a borrow is held. Child execution and completion
boundaries reject cancelled capabilities before mutation. A closed queue retains
the logical cancellation fence until a safe cleanup boundary.

The shared evaluator capability carries its exact external-load waiter in both
direct and queued execution. Cancelling one waiter leaves surviving siblings on
the original host attempt; a retired attempt cannot complete a replacement.
Unused plural completion adapters are removed. Synchronous callback success and
error results remain intact.

Verification: 570 native tests, focused cancellation/shared-load tests, WASM test
compilation and 15 browser fixtures pass. The final native-test-only change adds
a timeout and requires the sibling load result to be `Ok(Ok(()))`; that focused
test and WASM compilation pass afterward. The navigator reconstructed the earlier
eval.rs hash by reversing those two test edits, confirming unchanged production
source. Browser receipts were recovered from the pilot's terminal tool result;
the snapshot report alone is not used as proof of the 15-fixture result.

The remaining Stage 2.3 requirement is a complete async-producer and consumer
inventory with exactly-once lifecycle evidence, including all extension and
resource paths. These focused tests do not establish that broader acceptance.
