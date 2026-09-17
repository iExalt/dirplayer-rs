# JSON-lines v1 protocol fixtures

This directory contains small, synthetic wire fixtures for the versioned
JSON-lines protocol used by the parity workers. The fixture shape is derived
from the reference definitions in `childhood-redux`:

- `workspace/parity/src/protocol.rs` defines the envelope, operation, result,
  handle, input, and error tags.
- `workspace/parity/src/capture.rs` defines decoded RGBA and PCM validation.
- `workspace/parity/src/session.rs` defines session and object handle fields.
- `tools/parity/dirplayer_worker.mjs` defines the browser adapter's line
  transport and its adapter-specific restrictions.

The reference files are read-only provenance. This directory has no dependency
on that repository and does not contain a native worker.

`requests.jsonl`, `responses.jsonl`, and `errors.jsonl` are raw protocol lines.
They cover the v1 wire tags and use no game assets. Capture values are tiny
synthetic examples that exercise decoding and shape checks. They are not
runtime responses, native execution evidence, or fidelity baselines.

`compatibility.jsonl` contains test records for boundaries where a Rust
decoder, the v1 process client, and the current browser adapter can differ.
The Rust decoder result fields in this file were checked against the actual
reference crate; see [REFERENCE_CHECK.md](REFERENCE_CHECK.md) for the command,
output, and source hashes. They must not be inferred from the JavaScript
adapter.

## Validation

Run the dependency-free validator from this repository:

```sh
mise exec -- python3 tools/protocol-fixtures/validate.py
```

The same check is available as a repository task:

```sh
mise run test:protocol-fixtures
```

This validates all checked-in fixtures and runs transport-only tests for a
malformed, UTF-16, overflowing-number, timed-out, oversized, non-reading,
stderr-flooding responder with an inherited descendant. Boundary tests exercise
actual file and pipe readers at exactly 8 MiB and above, including UTF-8 bytes
and the trailing newline. These tests exercise the harness process boundary;
they do not validate a production runtime.

The worker mode accepts an executable and arguments without invoking a shell.
It sends `discover.jsonl` by default, so it never starts an asset-dependent
runtime implicitly:

```sh
mise exec -- python3 tools/protocol-fixtures/validate.py \
  --worker /path/to/native-worker
```

To select a different raw request stream, pass `--stream` before `--worker`:

```sh
mise exec -- python3 tools/protocol-fixtures/validate.py \
  --stream tools/protocol-fixtures/requests.jsonl \
  --worker /path/to/native-worker --worker-arg
```

The harness bounds each response read to 8 MiB, includes the newline in the
limit, enforces one deadline across nonblocking request write and response read,
drains bounded stderr, correlates response IDs, and cleans up an owned
process-group on POSIX. The transport harness is POSIX-only because it relies
on nonblocking pipes and process-group cleanup; fixture-only validation remains
portable Python.

## Compatibility notes

The canonical Rust envelope uses unsigned `u64` request IDs, so IDs above the
JavaScript safe-integer range remain valid Rust wire values. The current
browser worker rejects those IDs because it uses `Number.isSafeInteger`; it
also accepts negative safe integers that the Rust `u64` decoder rejects.

The Rust envelope structs deny unknown top-level fields. Nested operation
payloads, handles, capabilities, and structured errors accept extra fields in
the observed serde contract; the compatibility fixture records those cases.
Capture wire structs remain strict. `StructuredError` details and capture
`sha256` fields are optional on the wire. Capture hashes are ignored during
Rust deserialization and recomputed from decoded pixels or PCM, so a supplied
hash is not a canonical acceptance condition. Virtual-input f32 fields accept
serde overflow values such as `1e40`; PCM capture validation separately
requires finite samples.

`decode_response` structurally decodes the version field as `u16`; the process
client separately requires protocol version 1. The compatibility fixture keeps
those structural and v1 semantic checks distinct.

The protocol's grounded maximum encoded line size is 8 MiB (`8 * 1024 * 1024`)
including the trailing newline. Self-tests exercise actual file and pipe
readers at exactly the limit and above without checking in a multi-megabyte
fixture.
