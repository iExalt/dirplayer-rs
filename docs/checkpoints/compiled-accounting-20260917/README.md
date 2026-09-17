# Compiled runner accounting checkpoint

The local compiled runner now returns its exit reason and the number of backward
jumps taken during the invocation. Counts are retained for normal completion,
interpreter escape, and cooperative yield. Input polling still yields after one
backward jump; compute loops still yield at 4096. No fatal watchdog is added.

The navigator reviewed the one-file implementation and verified its live SHA-256
against `artifact-and-source.sha256`. Four named native tests passed from the
recorded emitted library test binary; their output and exit status are retained
here. This is accepted bounded accounting behavior at that source identity.

This checkpoint publishes only the bounded accounting change. Production driver integration,
active-frame breakpoint refresh, owner-qualified compilation caches, debugger
resume/cancellation, WASM validation and combined browser regression remain
outside this checkpoint. In particular, this does not establish that production
handler dispatch executes IR or close Stage 2.2.
