# Rust reference check

This is development evidence for the checked-in compatibility classifications.
The temporary harness and its generated Cargo target were removed after the
run. It used the actual `childhood-redux/workspace/parity` crate by path and
did not modify that checkout.

Reference checkout:

```text
revision: a7037761bd759f9d1515671ed792d12fb0208da8
workspace/parity/src/protocol.rs: 301f81557ae64fb4781f149473c92e708609ed06ab081c480ff37b8b173ee8ca
workspace/parity/src/capture.rs: 6f18578c0915ea92bb5e6dad8b538971446008a1b5fa5592a709f17b3a536146
workspace/parity/src/session.rs: 581a42ae22c4c5f9143a403d40e1a67452e30c4d3d7fcc8e95a3227785e9d33b
tools/parity/dirplayer_worker.mjs: 22b51434a5c7d0d87a5d5a9b7777422de8648d7f90fbb879e23cee993979f3a6
```

The temporary project used this manifest dependency and was run from the
DirPlayer checkout:

```toml
[dependencies]
parity = { path = "/Users/clliaw/Projects/childhood-redux/workspace/parity" }
serde_json = "=1.0.151"
```

The exact verification command was:

```sh
mise exec -- cargo run \
  --manifest-path /private/tmp/dirplayer-protocol-fixture-reference/Cargo.toml \
  --offline
```

The initial dependency resolution required one network-enabled run because the
temporary project had no lockfile. The command then completed successfully in
offline mode. Its output was:

```text
encoded_requests=20 decoded_responses=19
request_id_js_lower_boundary=rust_decoder_accept
request_id_js_safe_boundary=rust_decoder_accept
request_id_u64_max=rust_decoder_accept
request_id_negative=rust_decoder_reject
request_id_boolean=rust_decoder_reject
request_id_float_token=rust_decoder_reject
top_level_unknown_request_field=rust_decoder_reject
response_version_two=rust_decoder_accept
acknowledged_null_content=rust_decoder_accept
rgba_hash_omitted=rust_decoder_accept
rgba_hash_ignored=rust_decoder_accept
invalid_rgba_pixel_length=rust_decoder_reject
invalid_pcm_shape=rust_decoder_reject
nested_request_extra_field=rust_decoder_accept
unit_request_null_args=rust_decoder_accept
target_externally_tagged_root=rust_decoder_accept
target_externally_tagged_multiple=rust_decoder_reject
pointer_f32_overflow=rust_decoder_accept
session_handle_nested_extra=rust_decoder_accept
capabilities_nested_extra=rust_decoder_accept
structured_error_nested_extra=rust_decoder_accept
rgba_capture_unknown_extra=rust_decoder_reject
```

The checked-in Python validator independently executes those structural and v1
semantic verdicts from the observed serde contract. Browser-worker labels are
adapter observations from `dirplayer_worker.mjs`; they are not native-worker
results.

Final local validation output:

```text
$ mise exec -- python3 tools/protocol-fixtures/validate.py
validated 61 protocol fixtures and transport self-tests
$ mise exec -- python3 tools/protocol-fixtures/validate.py --no-self-tests
validated 61 protocol fixtures
```

The first command was also run outside the sandbox to exercise POSIX process
group cleanup for the non-reading child, large request write, stderr flood,
and inherited descendant case. The temporary reference harness was then
removed from `/private/tmp/dirplayer-protocol-fixture-reference`.

Reproduction recipe for the temporary `src/main.rs`: read each request line
from `discover.jsonl` and `requests.jsonl`, deserialize it as
`RequestEnvelope`, pass it through `encode_request`, and deserialize the
encoded bytes again; read each line from `responses.jsonl` and `errors.jsonl`
with `decode_response`; then read each compatibility record and dispatch its
`wire` string to `RequestEnvelope` or `decode_response` according to
`direction`, comparing success or failure with `rust_decoder`. Print one
`case=rust_decoder_accept|reject` line per compatibility record. This recipe
uses no game assets or runtime worker.
