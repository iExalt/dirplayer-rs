# Sound queue pitch and diagnostics

Sound-channel queue and playlist entries honor numeric `#rateShift` values in
semitones. The entry is resolved when playback starts and uses
`2 ** (rateShift / 12)` as its Web Audio playback rate. Missing rateShift resets
the rate to 1; malformed or nonfinite rates are rejected. Queueing another entry
does not retune a source already playing. The cue/current-time clock advances in
source time at the selected rate.

A repeated segment can reuse its decoded buffer and pitch. A transition to a new
entry resolves and decodes that entry, instead of blindly replaying the previous
entry's buffer. This may incur decode latency between distinct entries; it does
not establish sample-accurate or gapless playlist scheduling.

Missing members stay silent. Their diagnostics identify the 1-based channel,
inner member reference/type, and outer entry type. The player does not fabricate
replacement sound assets.

After a source successfully starts, the existing `onDebugMessage` callback emits
JSON with `event: "sound-playback-started"`, `schema_version: 1`, `channel`,
`member`, `playback_rate`, `buffer_duration`, and `loop_count`. Cached segment
repeats emit another event. These events describe scheduling, not audible output
or perceptual fidelity. Consumers should ignore unrelated debug messages and
unknown event/schema versions.

## Verification

```sh
mise exec rust@1.98.1 -- cargo test --manifest-path vm-rust/Cargo.toml --locked --lib sound_channel::
```

`scripts/test-sound-playback.mjs` exports
`testSoundPlayback(page, member, otherMember)` for a Playwright page with a loaded
Director movie and `window.vm`. Supply two distinct short sounds with different
buffer durations. The helper stops movie playback and exercises pitch 0/-2/+12,
queue and playlist transitions, repeated entries, direct-play reset, invalid
pitch rejection, and recovery after a missing member. The caller owns browser
launch, fixture serving, cleanup, and retaining the returned results.

Validated with Spybot's `s.select` and `s.begin`, plus the original title, music,
button-press, and START sequence. The accepted silent rollover and unrelated
missing `s.slam` remain diagnostics. No original game asset or script was edited.
