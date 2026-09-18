#!/usr/bin/env python3
"""Author deterministic AVM1 pointer fixtures from Ruffle's local button test.

The Ruffle button3 SWF supplies only the geometric button and placement records.
This generator replaces its frame DoAction with test-owned AVM1 handlers, patches
the background color, and emits uncompressed FWS files so the bytes are stable
and easy to inspect. No external asset is read.
"""

from __future__ import annotations

import hashlib
import os
import struct
import zlib
from pathlib import Path


ROOT = Path(__file__).resolve().parent
REPO = Path(os.environ["DIRPLAYER_REPO"]).resolve() if os.environ.get("DIRPLAYER_REPO") else ROOT.parents[2]
TEMPLATE = REPO / "ruffle/tests/tests/swfs/from_shumway/button3/test.swf"
OUT = ROOT


def u16(value: int) -> bytes:
    return struct.pack("<H", value)


def u32(value: int) -> bytes:
    return struct.pack("<I", value)


def action_header(opcode: int, payload: bytes = b"") -> bytes:
    if opcode < 0x80:
        if payload:
            raise ValueError(f"short action {opcode:#x} has payload")
        return bytes([opcode])
    return bytes([opcode]) + u16(len(payload)) + payload


def push_string(value: str) -> bytes:
    payload = b"\x00" + value.encode("ascii") + b"\x00"
    return action_header(0x96, payload)


def push_int(value: int) -> bytes:
    return action_header(0x96, b"\x07" + struct.pack("<i", value))


def set_variable(path: str, value: int) -> bytes:
    return push_string(path) + push_int(value) + bytes([0x1D])


def copy_variable(destination: str, source: str) -> bytes:
    return push_string(destination) + push_string(source) + bytes([0x1C, 0x1D])


def call_external(name: str) -> bytes:
    # ActionCallMethod pops method, object, argument count, then arguments.
    # ExternalInterface.call(name) therefore leaves the callback name, count,
    # and flash.external.ExternalInterface object on the stack in that order.
    return (
        push_string(name)
        + push_int(1)
        + push_string("flash.external.ExternalInterface")
        + bytes([0x1C])
        + push_string("call")
        + bytes([0x52, 0x17])
    )


def define_function(body: bytes) -> bytes:
    # DefineFunction's action length covers only name/params/code_length.
    # Ruffle's reader then extends the action span by code_length and reads the
    # body which follows the declared payload.
    payload = b"\x00" + u16(0) + u16(len(body))
    return action_header(0x9B, payload) + body


def assign_handler(target: str, name: str, body: bytes) -> bytes:
    return push_string(target) + bytes([0x1C]) + push_string(name) + define_function(body) + bytes([0x4F])


def frame_actions(owner: str, reentry: bool = False) -> bytes:
    moved = f"_root.{owner}Moved"
    pressed = f"_root.{owner}Pressed"
    saw_move = f"_root.{owner}PressSawMove"
    order = f"_root.{owner}Order"
    on_move = (
        (call_external("dirplayer_testResetOwnerDuringFlashMove") if reentry else b"")
        + set_variable(moved, 1)
        + set_variable(order, 1)
        + bytes([0x00])
    )
    on_press = (
        (call_external("dirplayer_testRecordFlashPress") if reentry else b"")
        + set_variable(pressed, 1)
        + copy_variable(saw_move, moved)
        + set_variable(order, 2)
        + bytes([0x00])
    )
    actions = (
        set_variable(moved, 0)
        + set_variable(pressed, 0)
        + set_variable(saw_move, 0)
        + set_variable(order, 0)
        # AVM1 buttons intentionally do not handle ClipEvent::MouseMove in
        # Ruffle; the anycast handler belongs to the root MovieClip.
        + assign_handler("_root", "onMouseMove", on_move)
        + assign_handler("btn", "onPress", on_press)
        # The source has one frame; stop it after initialization so a later
        # timeline tick cannot reset the owner counters before input arrives.
        + bytes([0x07])
        + bytes([0x00])
    )
    return actions


def read_rect_size(header: bytes) -> tuple[int, int, int]:
    class Bits:
        def __init__(self, data: bytes):
            self.data = data
            self.pos = 0

        def read(self, count: int) -> int:
            value = 0
            for bit in range(count):
                value = (value << 1) | ((self.data[(self.pos + bit) // 8] >> (7 - ((self.pos + bit) % 8))) & 1)
            self.pos += count
            return value

        def signed(self, count: int) -> int:
            value = self.read(count)
            return value - (1 << count) if count and value & (1 << (count - 1)) else value

    bits = Bits(header)
    nbits = bits.read(5)
    values = [bits.signed(nbits) for _ in range(4)]
    return (bits.pos + 7) // 8, values[1] - values[0], values[3] - values[2]


def encode_tag(code: int, payload: bytes) -> bytes:
    if len(payload) < 63:
        return u16((code << 6) | len(payload)) + payload
    return u16((code << 6) | 63) + u32(len(payload)) + payload


def parse_tags(data: bytes) -> tuple[bytes, list[tuple[int, bytes]]]:
    rect_len, _, _ = read_rect_size(data[8:])
    prefix_len = 8 + rect_len + 4
    prefix = data[:prefix_len]
    pos = prefix_len
    tags: list[tuple[int, bytes]] = []
    while True:
        raw = struct.unpack_from("<H", data, pos)[0]
        pos += 2
        code, length = raw >> 6, raw & 0x3F
        if length == 0x3F:
            length = struct.unpack_from("<I", data, pos)[0]
            pos += 4
        payload = data[pos : pos + length]
        if len(payload) != length:
            raise ValueError("truncated SWF tag")
        pos += length
        tags.append((code, payload))
        if code == 0:
            break
    if pos != len(data):
        raise ValueError("bytes remain after SWF End tag")
    return prefix, tags


def authored_swf(owner: str, rgb: tuple[int, int, int], reentry: bool = False) -> bytes:
    raw = TEMPLATE.read_bytes()
    if raw[:3] == b"CWS":
        raw = b"FWS" + raw[3:8] + zlib.decompress(raw[8:])
    if raw[:3] != b"FWS":
        raise ValueError("button3 template is not a SWF")
    prefix, tags = parse_tags(raw)
    found_action = False
    found_background = False
    rebuilt: list[tuple[int, bytes]] = []
    for code, payload in tags:
        if code == 12:
            if found_action:
                raise ValueError("template has multiple DoAction tags")
            payload = frame_actions(owner, reentry)
            found_action = True
        elif code == 9:
            if len(payload) != 3 or found_background:
                raise ValueError("template SetBackgroundColor shape changed")
            payload = bytes(rgb)
            found_background = True
        rebuilt.append((code, payload))
    if not found_action or not found_background:
        raise ValueError("template is missing DoAction or SetBackgroundColor")
    body = prefix + b"".join(encode_tag(code, payload) for code, payload in rebuilt)
    return body[:4] + u32(len(body)) + body[8:]


def write_fixture(owner: str, rgb: tuple[int, int, int], reentry: bool = False) -> Path:
    OUT.mkdir(parents=True, exist_ok=True)
    output = OUT / ("flash_mouse_reentry.swf" if reentry else f"flash_mouse_{owner}.swf")
    output.write_bytes(authored_swf(owner, rgb, reentry))
    return output


def main() -> None:
    outputs = [
        write_fixture("a", (0xC0, 0x20, 0x20)),
        write_fixture("b", (0x20, 0x40, 0xC0)),
        write_fixture("reentry", (0xC0, 0xA0, 0x20), reentry=True),
    ]
    for path in outputs:
        print(f"{path.name} {len(path.read_bytes())} {hashlib.sha256(path.read_bytes()).hexdigest()}")


if __name__ == "__main__":
    main()
