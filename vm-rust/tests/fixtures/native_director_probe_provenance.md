# Native Director probe provenance

`generate_native_director_probe.py` emits `native_director_probe.dcr` from
literal D5 records. It contains a DRCF configuration, a KEY* table, one
CAS*/CASt QuickDraw shape, one movie-script CASt, Lctx/Lnam/Lscr script
context, and a three-frame VWSC score. It has no external casts, Flash,
audio, fonts, nested movies, or Xtras.

The CASt record uses raw D5 member type 8 with a 17-byte ShapeInfo payload;
the native parser's shape disambiguation is the behavior under test. CAS* maps
movie-script member 1 before QuickDraw shape member 2, and each score frame
places member 2 at position `(4,4)` with size `24x24`, rendering the inset rect
`(4,4)-(28,28)`. Its score foreground is palette index 255 (black) and its
background is index 0 (white). A successful native snapshot therefore has a
white `(0,0)` corner, a black `(16,16)` center, and exactly 576 opaque black
pixels. The seven authored handlers (`prepareMovie`, `startMovie`,
`enterFrame`, `exitFrame`, `mouseDown`, `timerA`, and `timerB`) publish phase
and input globals, move the shape through `sprite(1).locH`, and exercise native
timeout callbacks. `prepareMovie` initializes `frameState` to `1`, and
`enterFrame` increments it. The zero-argument `mouseDown` handler reads the
canonical `the mouseH` and `the mouseV` values into `inputH` and `inputV`, then
sets `sprite(1).locH` from `mouseH`. A native `(8,8)` input therefore changes
the full RGBA rectangle from `(4,4)-(28,28)` to `(8,4)-(32,28)` without
advancing frame or logical time. `timerA` adds 10 and reschedules itself with
a 200-ms period; `timerB` multiplies `frameState` by 10 and forgets its
timeout argument. The noncommutative callback effects make the equal-deadline
registration order and timeout-before-frame ordering observable. The test
asserts complete constructed RGBA buffers before and after input, repeated
capture identity, `current_frame=1,2,3`, and `frameState=2,3,4` through the
owner-bound native presentation policy. The fixture has no browser timeout host
actions on this path.

The first contrast encoding swapped CAS* and inset geometry but left the D5
foreground byte at palette index 0, so the focused probe correctly observed a
white center and produced no PNG. The bounded second encoding set the authored
foreground to palette index 255 and encoded the inset as 24x24; it passed with
the exact corner, center, and 576-pixel assertions. This was a fixture color
mapping correction, not a native-host blocker.

The missing-resource follow-up should pass a missing primary DCR path to the
same local loader and record the expected `std::fs::read` boundary. It is kept
separate from this parser-valid proof so a missing file cannot be confused
with fixture parsing or native host behavior.

Regenerate and validate with:

```sh
mise exec -- python3 vm-rust/tests/fixtures/generate_native_director_probe.py
mise exec -- python3 vm-rust/tests/fixtures/validate_native_director_probe.py
```

The regenerated binary is 1649 bytes with SHA-256
`88b08a92b01e66dddd18b75be665dfaf0bb3b1de6967987d42a8fd082b1b516d`.
The generator is deterministic; the validator prints the SHA-256 used by the
probe evidence receipt. Timeout behavior beyond the canonical owned frame call
and all excluded resource families remain outside this fixture.
