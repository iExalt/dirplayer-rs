# Stage 2 closure inventory refresh

Read-only lexical scan of the current working tree while sprite-variable
verification runs. This is backlog evidence, not semantic classification,
runtime failure proof, or a completion percentage.

Command from `dirplayer-rs`:

```sh
mise exec -- python scripts/audit-runtime-ownership.py --format json
```

Full output is retained at
`~/Projects/.dirplayer-work/stage2-closure-audit/current-inventory.json`.
The report command exited zero; this was not the `--check` closure gate.

| Category | Findings |
| --- | ---: |
| legacy-accessor | 464 |
| legacy-task | 81 |
| rust-mutable-singleton | 55 |
| rust-static-review | 45 |
| rust-thread-local | 18 |
| rust-static-mut | 9 |
| rust-immutable | 3 |
| js-local-state-candidate | 191 |
| js-module-binding | 167 |
| js-declaration-review | 143 |
| js-module-registry | 52 |
| js-global-write | 16 |

The largest file totals include `player/mod.rs` (197), `lib.rs` (103),
`src/store/vmSlice.ts` (58), `flashPlayerManager.ts` (55), `player/events.rs` (50)
and `player/commands.rs` (33). These totals combine categories and may include
test or benign candidates. The sound-channel datum handler has 28 findings and
remains a useful bounded discovery target for the audio ownership gate.

Before Stage 2 acceptance, review each current production consumer, remove the
temporary adapters, rerun the classification and scanner closure gates against
the final source identity, and reproduce native/WASM/browser verification in
that same checkout. Current component tests cannot establish this broader gate.
