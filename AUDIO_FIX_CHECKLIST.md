# Audio fixes

- [x] Inspect pinned playback paths and retained Spybot failures.
- [x] Apply per-entry rateShift and improve invalid-member diagnostics.
- [x] Expose identified playback-start evidence after successful scheduling.
- [x] Run focused Rust and browser regressions, including queued-rate isolation.
- [x] Validate Spybot title/select/begin/music and correct harness reporting.

Verification: 6 native sound-channel tests, WebAssembly build, and browser
regressions plus the Spybot audio scenario. See docs/AUDIO_PLAYBACK.md.
