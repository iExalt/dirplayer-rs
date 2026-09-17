# Authored nested Flash fixture parser checkpoint

This receipt describes local, unpublished fixture and test sources at the hashes
below. The documentation checkpoint does not include those source changes;
the combined runtime and browser implementation remains under review.

The two authored Director containers pass the production
`read_director_file_bytes` entrypoint. The exact named integration test
`e2e::dirplayer_test_movies::nested::test_nested_flash_fixture_uses_production_director_parser`
executed **1 test, 0 failures**. An earlier short exact filter selected zero
tests and is not used as evidence. The retained output proves the named test ran.

The test checks normalized Director version 500, one cast entry, a KEY* table
and a VWSC score. `source.sha256` identifies the test and both container bytes;
all three hashes matched the live files when the navigator inspected the
receipt. `artifact.sha256` identifies the executed native integration binary.
The command in `compile-command.txt` builds that integration target with the
locked offline dependency graph and shared Cargo target. It does not execute
browser tests.

The generator independently reproduced all four local fixture files
byte-for-byte into a disposable directory, without modifying live build inputs.
The DCRs are 769 bytes with declared RIFX length 761. The SWFs are 48 bytes with
little-endian file lengths, one ShowFrame and terminal End, distinct authored
values 7/9, and backgrounds c02020/2040c0. Regeneration instructions and source
are in `vm-rust/tests/fixtures/nested_flash_fixture_provenance.md`.

This is parser feasibility evidence. It does not establish movie startup,
child command/event processing, Flash binding, actual Ruffle pixels, parent
composition, delayed publication, reset or failed-startup teardown. Those remain
required for the complete child Flash lifecycle gate. Concurrent child lifecycle
and browser fixture work is not accepted by this checkpoint.
