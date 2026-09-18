#!/usr/bin/env python3
"""Generate the deterministic AVM1 setCallback callback probe fixture.

The file is authored here rather than copied from an external SWF. It contains
one visible 32x32 shape and a frame DoAction stream that assigns a real
MovieClip method, _root.dirplayerCallbackProbe. The method accepts three named
parameters and copies them to root variables, which makes production
CallFunction argument delivery observable without an ExternalInterface hook.
"""

from __future__ import annotations

import hashlib
import struct
from pathlib import Path


ROOT = Path(__file__).resolve().parent
OUTPUT = ROOT / "flash_lingo_callback_probe.swf"


def le16(value: int) -> bytes:
    return struct.pack("<H", value)


def le32(value: int) -> bytes:
    return struct.pack("<I", value)


class BitWriter:
    def __init__(self) -> None:
        self.bits: list[int] = []

    def write(self, value: int, width: int, *, signed: bool = False) -> None:
        if signed and value < 0:
            value = (1 << width) + value
        if not 0 <= value < (1 << width):
            raise ValueError(f"{value} does not fit in {width} bits")
        self.bits.extend((value >> bit) & 1 for bit in range(width - 1, -1, -1))

    def finish(self) -> bytes:
        while len(self.bits) % 8:
            self.bits.append(0)
        result = bytearray()
        for offset in range(0, len(self.bits), 8):
            value = 0
            for bit in self.bits[offset : offset + 8]:
                value = (value << 1) | bit
            result.append(value)
        return bytes(result)


def rect_bits(left: int, top: int, right: int, bottom: int) -> bytes:
    width = max(abs(left), abs(top), abs(right), abs(bottom)).bit_length() + 1
    writer = BitWriter()
    writer.write(width, 5)
    for value in (left, right, top, bottom):
        writer.write(value, width, signed=True)
    return writer.finish()


def matrix_bits(tx: int, ty: int) -> bytes:
    width = max(abs(tx), abs(ty)).bit_length() + 1
    writer = BitWriter()
    writer.write(0, 1)  # HasScale
    writer.write(0, 1)  # HasRotate
    writer.write(width, 5)
    writer.write(tx, width, signed=True)
    writer.write(ty, width, signed=True)
    return writer.finish()


def action_header(opcode: int, payload: bytes = b"") -> bytes:
    if opcode < 0x80:
        if payload:
            raise ValueError(f"short action {opcode:#x} cannot carry a payload")
        return bytes((opcode,))
    return bytes((opcode,)) + le16(len(payload)) + payload


def push_string(value: str) -> bytes:
    return action_header(0x96, b"\x00" + value.encode("utf-8") + b"\x00")


def push_int(value: int) -> bytes:
    return action_header(0x96, b"\x07" + struct.pack("<i", value))


def push_null() -> bytes:
    return action_header(0x96, b"\x02")


def set_variable_int(path: str, value: int) -> bytes:
    return push_string(path) + push_int(value) + bytes((0x1D,))


def set_variable_string(path: str, value: str) -> bytes:
    return push_string(path) + push_string(value) + bytes((0x1D,))


def set_variable_null(path: str) -> bytes:
    return push_string(path) + push_null() + bytes((0x1D,))


def set_variable_type(path: str, source: str) -> bytes:
    return push_string(path) + push_string(source) + bytes((0x1C, 0x44, 0x1D))


def copy_parameter(path: str, parameter: str) -> bytes:
    return push_string(path) + push_string(parameter) + bytes((0x1C, 0x1D))


def push_parameter(parameter: str) -> bytes:
    return push_string(parameter) + bytes((0x1C,))


def increment_variable(path: str) -> bytes:
    return push_string(path) + push_string(path) + bytes((0x1C,)) + push_int(1) + bytes((0x0A, 0x1D))


def define_function(name: str, parameters: tuple[str, ...], body: bytes) -> bytes:
    payload = name.encode("utf-8") + b"\x00" + le16(len(parameters))
    payload += b"".join(parameter.encode("utf-8") + b"\x00" for parameter in parameters)
    payload += le16(len(body))
    # Ruffle's AVM1 reader treats the code bytes after the header-only action
    # payload as the function body. Keep the declared action length separate.
    return action_header(0x9B, payload) + body


def define_root_method(name: str, parameters: tuple[str, ...], body: bytes) -> bytes:
    # The frame DoAction runs in the root timeline scope. Ruffle's named
    # DefineFunction therefore installs the function as a root local binding.
    return define_function(name, parameters, body)


def callback_body() -> bytes:
    parameters = ("message_utf8", "score_number", "payload_object")
    return (
        copy_parameter("_root.callbackProbeString", parameters[0])
        + copy_parameter("_root.callbackProbeNumber", parameters[1])
        + copy_parameter("_root.callbackProbeObject", parameters[2])
        + increment_variable("_root.callbackProbeInvocationCount")
        + set_variable_int("_root.callbackProbeArgumentCount", 3)
        + set_variable_int("_root.callbackProbeDone", 1)
        + bytes((0x00,))
    )


def invoke_callback_body() -> bytes:
    return (
        increment_variable("_root.callbackWrapperInvocationCount")
        + push_string("_root")
        # ActionCallMethod pops arguments from the top of the AVM1 stack;
        # push them in reverse so pop_call_args yields message, number, root.
        + bytes((0x1C,))
        + push_parameter("score_number")
        + push_parameter("message_utf8")
        + push_int(3)
        + push_string("_root")
        + bytes((0x1C,))
        + push_string("dirplayerCallbackProbe")
        + bytes((0x52, 0x17, 0x00))  # CallMethod, Pop, End
    )


def encode_tag(code: int, payload: bytes) -> bytes:
    if len(payload) < 63:
        return le16((code << 6) | len(payload)) + payload
    return le16((code << 6) | 63) + le32(len(payload)) + payload


def visible_shape(rgb: tuple[int, int, int]) -> bytes:
    writer = BitWriter()
    writer.write(1, 4)  # NumFillBits
    writer.write(0, 4)  # NumLineBits
    writer.write(0, 1)  # StyleChangeRecord
    writer.write(0, 1)  # StateNewStyles
    writer.write(0, 1)  # StateLineStyle
    writer.write(1, 1)  # StateFillStyle1
    writer.write(0, 1)  # StateFillStyle0
    writer.write(1, 1)  # StateMoveTo
    writer.write(1, 5)  # MoveBits
    writer.write(0, 1, signed=True)
    writer.write(0, 1, signed=True)
    writer.write(1, 1)  # fill index 1
    for vertical, delta in ((False, 640), (True, 640), (False, -640), (True, -640)):
        writer.write(1, 1)  # StraightEdgeRecord
        writer.write(1, 1)  # StraightFlag
        writer.write(9, 4)  # NumBits = 11
        writer.write(0, 1)  # GeneralLineFlag
        writer.write(1 if vertical else 0, 1)  # VertLineFlag
        writer.write(delta, 11, signed=True)
    writer.write(0, 6)  # EndShapeRecord
    shape_records = writer.finish()
    shape = le16(1) + rect_bits(0, 0, 640, 640)
    shape += bytes((1, 0, *rgb, 0))  # one solid fill, no line styles
    return shape + shape_records


def place_shape() -> bytes:
    return bytes((0x06,)) + le16(1) + le16(1) + matrix_bits(0, 0)


def frame_actions() -> bytes:
    parameters = ("message_utf8", "score_number", "payload_object")
    return (
        set_variable_string("_root.callbackProbeString", "")
        + set_variable_int("_root.callbackProbeNumber", 0)
        + set_variable_null("_root.callbackProbeObject")
        + set_variable_int("_root.callbackProbeInvocationCount", 0)
        + set_variable_int("_root.callbackWrapperInvocationCount", 0)
        + set_variable_int("_root.callbackProbeArgumentCount", 0)
        + set_variable_int("_root.callbackProbeDone", 0)
        + set_variable_string("_root.callbackProbeEncoding", "café")
        + set_variable_string("_root.kind", "object")
        + define_root_method("dirplayerCallbackProbe", parameters, callback_body())
        + define_root_method("dirplayerInvokeCallbackProbe", parameters, invoke_callback_body())
        + set_variable_type("_root.callbackWrapperType", "_root.dirplayerInvokeCallbackProbe")
        + bytes((0x07, 0x00))  # Stop, End
    )


def authored_swf() -> bytes:
    stage = rect_bits(0, 0, 640, 640)
    tags = [
        (9, bytes((0x18, 0x18, 0x18))),
        (2, visible_shape((0x48, 0x90, 0xD8))),
        (26, place_shape()),
        (12, frame_actions()),
        (1, b""),
        (0, b""),
    ]
    body = stage + bytes((0x00, 0x0C, 0x01, 0x00))
    body += b"".join(encode_tag(code, payload) for code, payload in tags)
    return b"FWS" + bytes((9,)) + le32(len(body) + 8) + body


def main() -> None:
    data = authored_swf()
    OUTPUT.write_bytes(data)
    print(f"{OUTPUT.name} bytes={len(data)} sha256={hashlib.sha256(data).hexdigest()}")


if __name__ == "__main__":
    main()
