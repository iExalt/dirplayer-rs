# Spybot external cast audit

This audit records the external cast declarations in the recovered Spybot DCR,
the local candidate files, and the smallest safe contract for native title
startup. It does not claim that the local candidates contain the historical
level sound payloads.

## Findings

The DCR's `MCsL-223644` cast list declares five external libraries. Each entry
has the same recovered Windows path pattern, `sound_level_N.cst`, and the same
cast metadata:

| Declaration | Recovered MCsL path | id | preload | min/max |
| --- | --- | ---: | ---: | ---: |
| `sound_level_1` | `C:\\Documents and Settings\\Administrator\\Desktop\\SPYBOTS\\CODE\\phase_6_GOLD_FINAL\\807\\sound_level_1.cst` | 1024 | 0 | 1/0 |
| `sound_level_2` | `C:\\Documents and Settings\\Administrator\\Desktop\\SPYBOTS\\CODE\\phase_6_GOLD_FINAL\\807\\sound_level_2.cst` | 1024 | 0 | 1/0 |
| `sound_level_3` | `C:\\Documents and Settings\\Administrator\\Desktop\\SPYBOTS\\CODE\\phase_6_GOLD_FINAL\\807\\sound_level_3.cst` | 1024 | 0 | 1/0 |
| `sound_level_4` | `C:\\Documents and Settings\\Administrator\\Desktop\\SPYBOTS\\CODE\\phase_6_GOLD_FINAL\\807\\sound_level_4.cst` | 1024 | 0 | 1/0 |
| `sound_level_5` | `C:\\Documents and Settings\\Administrator\\Desktop\\SPYBOTS\\CODE\\phase_6_GOLD_FINAL\\807\\sound_level_5.cst` | 1024 | 0 | 1/0 |

The local recovered candidates are deliberately renamed `.cct` files. The
source-name-to-candidate mapping is:

| Declaration | Local candidate | Size | SHA-256 |
| --- | --- | ---: | --- |
| `sound_level_1.cst` | `resources/spybot/spybot-nightfall-incident-sound-level-1.cct` | 359 | `bd18e2325e2074a24b86db5a921b873e1f55bd38db7a7646a57eb0d929006035` |
| `sound_level_2.cst` | `resources/spybot/spybot-nightfall-incident-sound-level-2.cct` | 359 | `bd18e2325e2074a24b86db5a921b873e1f55bd38db7a7646a57eb0d929006035` |
| `sound_level_3.cst` | `resources/spybot/spybot-nightfall-incident-sound-level-3.cct` | 359 | `bd18e2325e2074a24b86db5a921b873e1f55bd38db7a7646a57eb0d929006035` |
| `sound_level_4.cst` | `resources/spybot/spybot-nightfall-incident-sound-level-4.cct` | 359 | `bd18e2325e2074a24b86db5a921b873e1f55bd38db7a7646a57eb0d929006035` |
| `sound_level_5.cst` | `resources/spybot/spybot-nightfall-incident-sound-level-5.cct` | 359 | `bd18e2325e2074a24b86db5a921b873e1f55bd38db7a7646a57eb0d929006035` |

The pinned recovery run converted each candidate to a regenerated `.cst` for
inspection. Every regenerated file is 876 bytes and has SHA-256
`a96eb63bc549d4bcf6e8afa704b9259412b19c12af1c34a5a01ef7c40516026c`. The
ProjectorRays logs identify the inputs as Macromedia Director 8.0#178 casts.
The shockwave extractor reports, for every regenerated cast: stage `0x0`,
tempo `0 fps`, zero bitmaps, zero sounds, and zero errors.

This is evidence of a valid, empty cast container. It is not evidence that the
file is an HTTP error body, and it is not evidence that it is the missing level
sound payload. Bounded local Flashpoint evidence adds the archive
`f256525e-afce-415f-81b9-0d862bc80580-1699829380483.zip`: it contains 30
relevant entries across `deu`, `eng`, `fra`, `jpn`, and `kor` (five 359-byte
`sound_level_N.cct` files plus one DCR per locale). The English
`sound_level_1.cct` has the expected `bd18e232...60035` hash, and the archive
contains no `snd_netload` entries. This is current local archive evidence for
the five same-size shell files and their locale distribution; it does not prove
that nonempty level payloads exist elsewhere.

## Source role split

The MCsL declarations use `.cst` names because they describe external cast
libraries in the original authoring project. The runtime sound script uses a
different naming layer:

* play mode sets `soundNetSourcePostFix` to `.cct` and defines
  `soundCasts` as `snd_netload_1` through `snd_netload_5`;
* `SndStartLoading` calls `preloadNetThing(moviePath & whichCast)`;
* after `netDone`, `SndCheckLoadStatus` assigns the downloaded file to
  `castLib("sound_level_" & N).fileName`;
* `SndFixCastLibraries` and `SndLoadCastLibraries` are authoring-mode paths,
  not the normal title startup path.

The actual nonempty `.cct` payloads requested by `snd_netload_N.cct` were not
recovered or qualified here. The five local 359-byte candidates are empty
containers and must not be presented as usable level sound data merely because
their names can be aliased.

Retained DirPlayer title evidence requests all five `sound_level_N` shells
immediately after the DCR and before `onMovieLoaded`/the title frame.
`preload=0` is eligible for the existing MovieLoaded/AfterFrameOne preload
pipeline. The source Lingo's `snd_netload_N.cct` payload requests happen later.
Therefore, absence of nonempty `snd_netload` payloads does not block a
declaration-aware title-start result once the five verified shell bytes have
been preloaded/applied. It does block any claim of external level sound
fidelity.

## Current native path

`TestPlayer::load_movie_quiet` in `vm-rust/src/player/testing.rs` parses the DCR,
collects every nonempty `cast_entries.file_path`, and calls
`validate_native_movie_features` before loading the movie. The unqualified path
continues to reject all external casts, preserving the P0 unsupported result.

If the guard passes, `load_movie_from_dir_owned` reaches the normal
`DirPlayer::load_movie_from_dir_sync` and `CastManager::load_from_dir` path.
External entries intentionally skip embedded CAS matching. Existing
`normalize_cast_lib_path` in `vm-rust/src/player/cast_manager.rs` replaces
Windows separators, keeps the basename, converts the extension to `.cct`, and
joins it to the movie base URL.

The native worker now accepts an optional strict `resource_aliases` map. It
derives the required normalized `.cct` keys from the DCR declarations, requires
the supplied map to be an exact match, and validates each relative path,
canonical target, hash, and Director signature before constructing the player.
It then applies those already-validated bytes through the existing
owner/capability/session completion path before movie initialization. Omitting
the map still takes the previous rejection path.

## Smallest fail-closed contract

The implementation should be limited to the native worker's external-cast
qualification and its root-confined resource lookup. Exact likely file
boundaries are:

* `vm-rust/src/native_parity_worker.rs` for schema, root, hash, and signature
  validation plus deterministic pre-init load orchestration;
* `vm-rust/src/player/testing.rs` for a typed qualified external-cast input
  replacing blanket rejection;
* `vm-rust/src/player/session.rs` only if needed to expose the existing
  prepare/apply owner and capability pipeline;
* `vm-rust/src/player/cast_manager.rs` and `cast_lib.rs` remain unchanged unless
  a separate proved loader defect emerges.

The childhood-redux P0 harness/config would only supply alias and hash data in
a separately assigned run.

1. Parse external declarations generically and normalize Windows drive/path
   syntax to their basenames. For this fixture, the derived exact set is the
   five known `sound_level_N.cct` keys.
2. Extend the native worker adapter with a strict `resource_aliases` object
   keyed by normalized `.cct` name. Each value carries one relative `path` and
   one lowercase `sha256`. Resolve the path beneath the canonical
   `resource_root`. Reject absolute POSIX or Windows paths, `..` traversal in
   mixed separators, symlink escapes, missing files, and non-regular files.
3. Reject repeated candidate targets, unknown alias keys, and missing required
   aliases. The required keys for this fixture are `sound_level_1.cct` through
   `sound_level_5.cct`.
4. Verify the candidate's expected hash and Director cast signature before
   initialization. The five verified 359-byte shell bytes must be preloaded and
   applied through the existing owner/capability/session machinery before
   movie initialization; merely deferring all shells is insufficient.
5. Preserve the existing loader and basename normalization, but supply a
   root-confined alias lookup for the verified candidate. Keep deterministic
   validation and shell application before MovieLoaded/AfterFrameOne and before
   title initialization is reported ready.
6. Treat a later `snd_netload_N.cct` request as a separate resource lookup. An
   absent alias is an explicit missing-resource failure; a future recovered
   candidate must pass the same root, hash, and cast validation. If it parses
   as an empty cast, later member lookup remains empty. Do not silently
   substitute an embedded sound or treat a shell as a qualified level payload.

## Targeted test matrix

Tests should cover valid five-declaration resolution; Windows path aliasing;
absolute path, traversal, and symlink escape; missing and non-regular files;
repeated targets, unknown aliases, and unknown declarations; hash and signature
mismatch; qualified-shell-before-title startup ordering; a later missing
`snd_netload` alias; and later sound-member lookup failing against an empty
cast. Existing guard tests should continue to reject undeclared external casts.

This audit does not authorize changes to Flash, input, PCM, destination, the
P0 receipt, or the loader implementation.

## Implemented and verified boundary

The generic qualification path is implemented in
`vm-rust/src/native_parity_worker.rs` and the pre-init application path in
`vm-rust/src/player/testing.rs`. No cast-manager, cast-library, session, or
network-loader change was needed. The production qualification value is opaque:
its fields and constructor remain private, and only the worker's root, hash,
signature, and exact-set validator can construct it. A test-only constructor is
compiled only under `cfg(test)` to exercise the owner pipeline with the existing
Director probe fixture.

Focused verification covers arbitrary Windows declaration normalization; one-
and five-key exact sets; missing and extra aliases; absolute, Windows-drive,
mixed-separator traversal, missing, directory, and symlink-escape paths;
duplicate canonical targets; malformed and mismatched hashes; invalid Director
signatures/parser failures; the unchanged unqualified external-cast rejection;
and qualified application reaching `Loaded` casts plus a `Ready` preload barrier
before movie initialization. The existing worker wire test omits
`resource_aliases`, proving the optional schema preserves prior clients.

This verification does not rerun Spybot. Its embedded Flash member remains the
next expected feature gate once childhood-redux supplies the five exact aliases
in a separately assigned probe. Later `snd_netload_N.cct` remains unsupported:
this change adds no general network lookup and does not qualify the absent level
sound payloads.
