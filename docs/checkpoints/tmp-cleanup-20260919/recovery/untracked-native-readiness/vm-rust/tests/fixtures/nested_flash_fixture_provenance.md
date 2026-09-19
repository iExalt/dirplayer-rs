# Nested Flash fixture provenance

This staging directory contains two deterministic, authored test fixtures. The
SWFs are derived only from the checked-in 43-byte `flash_initial_access.swf`
test asset: each keeps its tiny `_root.fixture` ActionScript assignment, adds a
`SetBackgroundColor` tag, and adds a placed opaque 32x32 solid shape. Owner A
uses value `7` and RGB `c02020`; owner B uses value `9` and RGB `2040c0`. The
shape uses the same owner-specific RGB, so transparent Ruffle `wmode` cannot
make composition readback black.

`nested_flash_bad.dcr` reuses the valid Director envelope but replaces its
embedded SWF with a fixed `BAD!` payload. This is only a deterministic
malformed-SWF input; the post-registration lifecycle rollback assertion is
driven separately by the real callback registrar failure hook.

Each SWF is embedded as a raw `SWF ` child chunk in a minimal uncompressed
RIFX/MV93 Director container. The container uses the parser's current D5 path:

| Chunk id | FourCC | Purpose |
| ---: | --- | --- |
| 0 | `DRCF` | 32x32 stage, one cast member, raw file version 1201 (human D5/500) |
| 1 | `KEY*` | CAS*, CASt, SWF-child, and VWSC ownership entries |
| 2 | `CAS*` | Cast member list containing member section 3 |
| 3 | `CASt` | Modern Flash member (`raw type 8`) with deterministic `FLSH` metadata |
| 4 | `SWF ` | The owner-specific SWF payload with a visible DefineShape/PlaceObject2 pair |
| 5 | `VWSC` | One D5 frame with Flash member 1 on a visible sprite channel |

`generate_nested_flash_fixture.py` is the checked-in source of the bytes. From
the repository root, regenerate and validate with:

```sh
python3 vm-rust/tests/fixtures/generate_nested_flash_fixture.py
python3 vm-rust/tests/fixtures/validate_nested_flash_fixture.py
```

The validator requires exactly one owner-colored `DefineShape` rectangle with
stage bounds 0..640 twips, exactly one `PlaceObject2` at depth 1 referring to
shape 1 at the origin, and the matching `_root.fixture` value in the action
stream. The malformed fixture intentionally skips these SWF checks because its
embedded payload is `BAD!`.

The production route remains mandatory:
`NestedMovieStart::parse` -> `start_nested_movie_owned` -> `load_movie_from_dir_owned`.
