#!/usr/bin/env python3
"""Structural validator for the generated AVM1 pointer fixtures."""

from __future__ import annotations

import hashlib
import struct
from pathlib import Path

from generate_flash_pointer_fixture import OUT, parse_tags, push_int, push_string, read_rect_size


def parse_actions(data: bytes) -> list[tuple[int, bytes, bytes]]:
    actions = []
    pos = 0
    while pos < len(data):
        opcode = data[pos]
        pos += 1
        if opcode == 0:
            actions.append((opcode, b"", b""))
            if pos != len(data):
                raise ValueError("bytes follow action End")
            break
        if opcode < 0x80:
            payload = b""
        else:
            if pos + 2 > len(data):
                raise ValueError("truncated action length")
            length = struct.unpack_from("<H", data, pos)[0]
            pos += 2
            payload = data[pos : pos + length]
            if len(payload) != length:
                raise ValueError("truncated action payload")
            pos += length
        body = b""
        if opcode == 0x9B:
            # DefineFunction's action length covers name/params/code_length;
            # code_length bytes follow the declared payload.
            if len(payload) != 5:
                raise ValueError("DefineFunction action length includes its body")
            body_len = struct.unpack_from("<H", payload, 3)[0]
            body = data[pos : pos + body_len]
            if len(body) != body_len:
                raise ValueError("truncated DefineFunction body")
            pos += body_len
        actions.append((opcode, payload, body))
    if not actions or actions[-1][0] != 0:
        raise ValueError("action stream is not terminated")
    return actions


def push_string_value(payload: bytes) -> str | None:
    if len(payload) < 2 or payload[0] != 0 or payload[-1] != 0:
        return None
    return payload[1:-1].decode("ascii")


def push_int_value(payload: bytes) -> int | None:
    if len(payload) != 5 or payload[0] != 7:
        return None
    return struct.unpack_from("<i", payload, 1)[0]


def external_call_names(action: bytes) -> list[str]:
    calls = []
    actions = parse_actions(action)
    for index in range(len(actions) - 6):
        window = actions[index : index + 7]
        if [opcode for opcode, _, _ in window] != [0x96, 0x96, 0x96, 0x1C, 0x96, 0x52, 0x17]:
            continue
        callback = push_string_value(window[0][1])
        count = push_int_value(window[1][1])
        interface = push_string_value(window[2][1])
        method = push_string_value(window[4][1])
        if callback is None or count != 1 or interface != "flash.external.ExternalInterface" or method != "call":
            raise ValueError("malformed ExternalInterface.call stack sequence")
        calls.append(callback)
    return calls


def external_call_encoding(name: str) -> bytes:
    return (
        push_string(name)
        + push_int(1)
        + push_string("flash.external.ExternalInterface")
        + bytes([0x1C])
        + push_string("call")
        + bytes([0x52, 0x17])
    )


def validate_reentry_action(action: bytes, owner: str) -> None:
    actions = parse_actions(action)
    functions = [(payload, body) for opcode, payload, body in actions if opcode == 0x9B]
    if len(functions) != 2:
        raise ValueError("reentry fixture expected two handler functions")
    calls = [external_call_names(body) for _, body in functions]
    flattened = [name for names in calls for name in names]
    expected = ["dirplayer_testResetOwnerDuringFlashMove", "dirplayer_testRecordFlashPress"]
    if flattened != expected:
        raise ValueError(f"reentry fixture external hooks differ: {flattened!r}")
    if any(len(names) != 1 for names in calls):
        raise ValueError("reentry handler must contain exactly one external call")
    for payload, body in functions:
        body_actions = parse_actions(body)
        if not body or body[-1] != 0 or 0x1D not in [opcode for opcode, _, _ in body_actions]:
            raise ValueError("reentry handler does not retain its state write")


def validate(path: Path, owner: str, reentry: bool = False) -> str:
    data = path.read_bytes()
    if data[:3] != b"FWS" or data[3] != 17:
        raise ValueError(f"{path.name}: expected FWS version 17")
    if struct.unpack_from("<I", data, 4)[0] != len(data):
        raise ValueError(f"{path.name}: FileLength mismatch")
    rect_len, width, height = read_rect_size(data[8:])
    if (width, height) != (11_000, 8_000):
        raise ValueError(f"{path.name}: expected 550x400 twips, got {width}x{height}")
    if struct.unpack_from("<H", data, 8 + rect_len + 2)[0] != 1:
        raise ValueError(f"{path.name}: expected one frame")
    _, tags = parse_tags(data)
    codes = [code for code, _ in tags]
    for required in (9, 12, 34, 1, 0):
        if codes.count(required) != 1:
            raise ValueError(f"{path.name}: tag {required} count is {codes.count(required)}")
    if codes.count(26) < 1:
        raise ValueError(f"{path.name}: no PlaceObject2 tag")
    background = next(payload for code, payload in tags if code == 9)
    action = next(payload for code, payload in tags if code == 12)
    actions = parse_actions(action)
    functions = [(payload, body) for opcode, payload, body in actions if opcode == 0x9B]
    if len(functions) != 2:
        raise ValueError(f"{path.name}: expected two handler functions")
    expected = [
        f"_root.{owner}Moved",
        f"_root.{owner}Pressed",
        f"_root.{owner}PressSawMove",
        f"_root.{owner}Order",
    ]
    for text in expected:
        if text.encode("ascii") + b"\0" not in action:
            raise ValueError(f"{path.name}: missing {text}")
    for text in ("_root", "btn", "onMouseMove", "onPress"):
        if text.encode("ascii") + b"\0" not in action:
            raise ValueError(f"{path.name}: missing handler target/name {text}")
    opcodes = [opcode for opcode, _, _ in actions]
    function_indexes = [index for index, opcode in enumerate(opcodes) if opcode == 0x9B]
    if len(function_indexes) != 2:
        raise ValueError(f"{path.name}: expected two DefineFunction actions")
    if opcodes.count(0x4F) < 2:
        raise ValueError(f"{path.name}: handlers are not followed by SetMember")
    if opcodes.count(0x07) != 1 or opcodes[-2:] != [0x07, 0x00]:
        raise ValueError(f"{path.name}: frame action stream is not stopped before End")
    for index in function_indexes:
        if index < 3 or opcodes[index - 3 : index] != [0x96, 0x1C, 0x96] or index + 1 >= len(opcodes) or opcodes[index + 1] != 0x4F:
            raise ValueError(f"{path.name}: malformed handler assignment around DefineFunction")
    for payload, body in functions:
        if len(payload) != 5 or payload[0] != 0 or struct.unpack_from("<H", payload, 1)[0] != 0:
            raise ValueError(f"{path.name}: malformed function header")
        body_len = struct.unpack_from("<H", payload, 3)[0]
        if body_len != len(body) or not body or body[-1] != 0:
            raise ValueError(f"{path.name}: malformed function body")
        body_actions = parse_actions(body)
        if 0x1D not in [opcode for opcode, _, _ in body_actions]:
            raise ValueError(f"{path.name}: handler does not write a variable")
    if reentry:
        validate_reentry_action(action, owner)
    elif external_call_names(action):
        raise ValueError(f"{path.name}: ordinary fixture unexpectedly calls ExternalInterface")
    if len(background) != 3:
        raise ValueError(f"{path.name}: malformed background tag")
    digest = hashlib.sha256(data).hexdigest()
    print(f"{path.name} ok bytes={len(data)} rect=550x400 background={background.hex()} sha256={digest}")
    return digest


def reject_old_define_function_encoding(path: Path) -> None:
    """Ensure the former body-in-action-length layout is rejected."""
    data = path.read_bytes()
    _, tags = parse_tags(data)
    action = bytearray(next(payload for code, payload in tags if code == 12))
    pos = action.index(0x9B)
    declared = struct.unpack_from("<H", action, pos + 1)[0]
    header = action[pos + 3 : pos + 3 + declared]
    body_len = struct.unpack_from("<H", header, 3)[0]
    action[pos + 1 : pos + 3] = struct.pack("<H", declared + body_len)
    try:
        parse_actions(bytes(action))
    except ValueError:
        return
    raise ValueError(f"{path.name}: old body-in-action-length encoding was accepted")


def reject_bad_external_call_binding(path: Path, owner: str) -> None:
    data = path.read_bytes()
    _, tags = parse_tags(data)
    action = bytearray(next(payload for code, payload in tags if code == 12))
    encoded_call = external_call_encoding("dirplayer_testResetOwnerDuringFlashMove")
    call_pos = action.index(encoded_call) + len(encoded_call) - 2
    if action[call_pos] != 0x52:
        raise ValueError(f"{path.name}: selected external call does not decode as CallMethod")
    action[call_pos] = 0x4F
    try:
        validate_reentry_action(bytes(action), owner)
    except ValueError:
        return
    raise ValueError(f"{path.name}: malformed CallMethod binding was accepted")


def main() -> None:
    for owner in ("a", "b"):
        path = OUT / f"flash_mouse_{owner}.swf"
        validate(path, owner)
        reject_old_define_function_encoding(path)
        print(f"{path.name} rejects former body-in-action-length encoding")
    reentry = OUT / "flash_mouse_reentry.swf"
    validate(reentry, "reentry", reentry=True)
    reject_old_define_function_encoding(reentry)
    print(f"{reentry.name} rejects former body-in-action-length encoding")
    reject_bad_external_call_binding(reentry, "reentry")
    print(f"{reentry.name} rejects malformed ExternalInterface.call binding")


if __name__ == "__main__":
    main()
