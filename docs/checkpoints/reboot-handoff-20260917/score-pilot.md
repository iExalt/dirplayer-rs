# Score owner migration handoff

Status: PAUSED before compile per parent instruction. No Cargo/build launched by this increment.

Files touched by this increment:
- vm-rust/src/player/score.rs: copied the old Score::begin_sprites body into DirPlayer::begin_score_sprites, added explicit-player create_behavior_owned and owned_score helpers, replaced owned-path reserve calls with local owner_ref/owner_mut direct callbacks, removed the old Score::begin_sprites body.
- vm-rust/src/player/mod.rs: changed begin_all_sprites and direct film-loop call sites to begin_score_sprites. Existing unrelated WIP was already present.
- vm-rust/src/player/nested.rs: no edits in this increment; existing diagnostic test/WIP remains.

Current state is source-partial and compile-unverified. Known remaining work:
- finish borrow-safe score snapshot/apply/commit boundaries where a mutable selected-score sprite borrow overlaps sprite_set_prop, sound iteration, or recursive film-loop calls;
- replace the recursive film-loop block with a snapshot of member refs/frames followed by explicit player calls;
- update the remaining initial-load and owned-init call sites as required after review;
- audit all transitive helpers for ambient reserve access;
- add/retain mounted, colliding-session, Stage side-effect, and FilmLoop relative-cast/behavior tests;
- only then remove any compatibility residue and run diff-check/compile under lead coordination.

No formatter, Cargo, WASM, browser, commit, or push was run by this increment.
