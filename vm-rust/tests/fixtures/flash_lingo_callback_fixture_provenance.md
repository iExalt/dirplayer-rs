# AVM1 callback probe fixture provenance

flash_lingo_callback_probe.swf is a deterministic, test-owned AVM1 fixture
authored by generate_flash_lingo_callback_fixture.py. It does not copy or
depend on an external SWF asset.

The uncompressed FWS version 9 movie has a 32x32 pixel stage, one frame, one
opaque 32x32 shape at depth 1, and a frame DoAction stream. The action stream
assigns two real functions to _root, each with three parameters: message_utf8,
score_number, and payload_object. The frame DoAction defines both methods by
name in the root timeline scope. Ruffle's action_define_function uses
define_local for a named function, so the root timeline publishes both
callables without a SetMember target sequence. The host entry
dirplayerInvokeCallbackProbe reads the UTF-8 and numeric parameters, pushes
_root itself as the third object argument, pushes argument count 3, then looks
up _root and dirplayerCallbackProbe as the receiver and method. The wrapper
increments callbackWrapperInvocationCount before preparing its call. The frame
then probes the named root binding with Push
"_root.callbackWrapperType", Push
"_root.dirplayerInvokeCallbackProbe", GetVariable, TypeOf, and SetVariable;
the resulting callbackWrapperType observes the installed callable directly.
Because AVM1 ActionCallMethod pops method, receiver, argument count, then
arguments from the top of the stack, the wrapper pushes the object, number, and
UTF-8 string in reverse order so the callee receives message_utf8,
score_number, and payload_object. It executes AVM1 CallMethod and Pop, forcing
the inner registered method through Object::call_method
instead of the direct Player::call_function path. The registered
dirplayerCallbackProbe method reads and stores each argument, increments
callbackProbeInvocationCount, stores a canonical callbackProbeArgumentCount of
3, and sets callbackProbeDone. The separate wrapper counter distinguishes
entry into the host wrapper from execution of the registered inner method. The
authored _root.kind value is the string
object, so the received third object argument has an observable property. A
UTF-8 café sentinel exercises the fixture string encoding. The frame
initializes observable globals and stops before the terminal End action.

The production callback sequence is expected to be:

load SWF -> Lingo sprite.setCallback(_root, "dirplayerCallbackProbe", ...)
-> production Ruffle CallFunction(_root.dirplayerInvokeCallbackProbe, string, number, object)
-> AVM1 wrapper CallMethod(_root, "dirplayerCallbackProbe", ...)
-> Object::call_method -> captured owner/sprite/generation callback

The fixture intentionally contains no ExternalInterface call, pointer hook,
direct callback global, or VM/frontend test transport. The validator
independently parses the SWF header, RECT, tags, DefineShape/PlaceObject2
records, AVM1 action lengths, anonymous DefineFunction header/body split,
named root DefineFunction actions, absence of SetMember installation, the
exact post-definition GetVariable/TypeOf/SetVariable probe, parameter reads,
the reverse argument push sequence, and all three argument copies.

Regenerate and validate from the repository root:

python3 vm-rust/tests/fixtures/generate_flash_lingo_callback_fixture.py
python3 vm-rust/tests/fixtures/validate_flash_lingo_callback_fixture.py
