# Pointer fixture provenance

`flash_mouse_a.swf`, `flash_mouse_b.swf`, and `flash_mouse_reentry.swf` are
deterministic test-owned AVM1 fixtures. The geometry, button definition,
placement, and 550x400 stage come
from the repository-local Ruffle test asset:

`ruffle/tests/tests/swfs/from_shumway/button3/test.swf`

The template SHA256 is
`eb0608f67def768886015fefec5875868dd38db65b6e11138bbbe02c2b6db91b`.
It is a 762-byte CWS asset that inflates to 1643 bytes before rewriting.

The generator inflates the checked-in CWS asset, preserves its button and shape
records, replaces its frame `DoAction` with two test-owned handlers, patches the
`SetBackgroundColor` tag, emits an uncompressed FWS, and rewrites `FileLength`.
The generated frame contains one `Stop` action before `End`, so the one-frame
initialization does not run again on a later timeline tick between pointer
events.
The `onMouseMove` handler is installed on `_root` because Ruffle's AVM1 button
dispatcher intentionally excludes `ClipEvent::MouseMove`; `onPress` remains on
the button. The handlers initialize four `_root` globals, set `*Moved` and
`*Order=1` on move, then set `*Pressed`, copy `*Moved` into `*PressSawMove`, and
set `*Order=2` on press. Thus `PressSawMove == 1` proves the real move preceded
the press; two booleans alone are insufficient.

The AVM1 `DefineFunction` action uses Ruffle's header-only action length: the
declared length covers name/parameters/code-size, and the code bytes follow it.
The validator recursively checks both function bodies and rejects the former
body-in-action-length encoding.

`flash_mouse_reentry.swf` uses the same authored geometry with two additional
test hooks. Its root `onMouseMove` calls
`ExternalInterface.call("dirplayer_testResetOwnerDuringFlashMove")`, and its
button `onPress` calls
`ExternalInterface.call("dirplayer_testRecordFlashPress")`. The exact AVM1
stack sequence is `Push(name)`, `PushInt(1)`,
`Push("flash.external.ExternalInterface")`, `GetVariable`, `Push("call")`,
`CallMethod`, and `Pop`; the validator checks this sequence and rejects a
mutated `CallMethod` binding. The armed run must use a fresh Ruffle instance
after the positive-control run so a previously pressed button cannot suppress
the second press independently of the owner-generation fence.

The original local Ruffle input clicks `(250, 200)` and reports `Button clicked`.
Its RECT is `[0, 11000, 0, 8000]` twips, or 550x400 pixels. The browser test
must retain that geometry or map coordinates explicitly; `(250, 200)` is not a
safe point for a 300x200 sprite.

Generate and validate from the repository root with:

```text
python3 vm-rust/tests/fixtures/generate_flash_pointer_fixture.py
python3 vm-rust/tests/fixtures/validate_flash_pointer_fixture.py
```

The generator derives the repository root from its own location. Set
`DIRPLAYER_REPO` only when running it from a copied fixture directory.

Accepted artifact hashes:

```text
generate_flash_pointer_fixture.py cf65425b500eeac40bcf344efe988d5e2e0e7784df73bb4815e71590322c9c77
validate_flash_pointer_fixture.py 523882fc5c37692fd0d9be9fee7d09626c2a1735d22caa1f0abb070c26a19d3e
flash_mouse_a.swf 2112fdcd4153807408c02d3b72aeae08ca376787ca8a5a32aa98f5f6edf9c44d
flash_mouse_b.swf 60c316ccea5c30301d6244d3123331b583bde6451498dedcd71897cff982f118
flash_mouse_reentry.swf 26ecc422a5ad7d7ac7a392a58e4e9f61ef2f1ed67c9bb45b1d56dd27c030f268
```

## Reset reentry boundary

Resetting A after a completed click only proves stale-owner rejection at the
next command boundary. It does not prove the move-to-down fence inside one
`MouseDown` command. To cover that boundary without changing production code,
use a separate reentry variant whose `onMouseMove` calls the real Ruffle
`ExternalInterface.call("dirplayer_testResetOwnerDuringFlashMove")`. The browser
test installs that name as a test-owned WASM closure over A's
`BrowserPlayerHandle`; the closure calls the existing public reset operation.
The same SWF's `onPress` calls a second external hook that increments an
independent host-side press counter. First run a positive-control click with the
reset hook disarmed and assert that counter increments. Then arm the reset hook,
send the move, and assert that the move hook reset A synchronously, the stale
owned read is rejected, and the independent press counter remains zero because
the generation fence prevented the down event. B's move/press counter must remain
functional. Do not infer this from A's retired `Pressed` variable, which is no
longer a valid read after reset.

Ruffle's `JavascriptInterface`/`callExternalInterface` path is synchronous and
supports reentry through its existing context slot. The provider is installed
only when `allowScriptAccess` is true and networking is `All`; the manager sets
the former and the Ruffle defaults set the latter. If the production Ruffle
instance is not configured with this provider, keep the reentry test blocked
rather than claiming reset-between-events coverage from the ordinary post-click
reset case.
