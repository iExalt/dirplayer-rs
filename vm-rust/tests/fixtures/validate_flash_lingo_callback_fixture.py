#!/usr/bin/env python3
"""Independently validate the authored AVM1 callback probe SWF."""

from __future__ import annotations

import hashlib
import struct
from pathlib import Path


ROOT = Path(__file__).resolve().parent
INPUT = ROOT / "flash_lingo_callback_probe.swf"


class BitReader:
    def __init__(self, data: bytes) -> None:
        self.data = data
        self.position = 0

    def read(self, width: int) -> int:
        if self.position + width > len(self.data) * 8:
            raise ValueError("truncated bit field")
        value = 0
        for _ in range(width):
            value = (value << 1) | ((self.data[self.position // 8] >> (7 - self.position % 8)) & 1)
            self.position += 1
        return value

    def signed(self, width: int) -> int:
        value = self.read(width)
        return value - (1 << width) if value & (1 << (width - 1)) else value

    def align(self) -> None:
        self.position = (self.position + 7) & ~7


def le16(data: bytes, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def le32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def read_rect(data: bytes) -> tuple[list[int], int]:
    reader = BitReader(data)
    width = reader.read(5)
    values = [reader.signed(width) for _ in range(4)]
    reader.align()
    return values, reader.position // 8


def read_cstring(data: bytes, offset: int) -> tuple[str, int]:
    end = data.find(b"\x00", offset)
    if end < 0:
        raise ValueError("unterminated AVM1 string")
    try:
        value = data[offset:end].decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValueError("AVM1 string is not UTF-8") from error
    return value, end + 1


def parse_push(payload: bytes) -> list[tuple[str, object]]:
    values: list[tuple[str, object]] = []
    position = 0
    while position < len(payload):
        kind = payload[position]
        position += 1
        if kind == 0:
            value, position = read_cstring(payload, position)
            values.append(("string", value))
        elif kind == 2:
            values.append(("null", None))
        elif kind == 7:
            if position + 4 > len(payload):
                raise ValueError("truncated Push integer")
            values.append(("int", struct.unpack_from("<i", payload, position)[0]))
            position += 4
        else:
            raise ValueError(f"unsupported Push type {kind:#x}")
    if position != len(payload):
        raise ValueError("Push payload has trailing bytes")
    return values


def parse_actions(data: bytes) -> list[dict[str, object]]:
    actions: list[dict[str, object]] = []
    position = 0
    while position < len(data):
        opcode = data[position]
        position += 1
        if opcode == 0:
            actions.append({"opcode": opcode})
            if position != len(data):
                raise ValueError("bytes follow AVM1 End action")
            return actions
        payload = b""
        if opcode >= 0x80:
            if position + 2 > len(data):
                raise ValueError("truncated AVM1 action length")
            length = le16(data, position)
            position += 2
            payload = data[position : position + length]
            if len(payload) != length:
                raise ValueError("truncated AVM1 action payload")
            position += length
        action: dict[str, object] = {"opcode": opcode, "payload": payload}
        if opcode == 0x96:
            action["values"] = parse_push(payload)
        elif opcode == 0x9B:
            name, header_position = read_cstring(payload, 0)
            if header_position + 2 > len(payload):
                raise ValueError("truncated DefineFunction parameter count")
            parameter_count = le16(payload, header_position)
            header_position += 2
            parameters: list[str] = []
            for _ in range(parameter_count):
                parameter, header_position = read_cstring(payload, header_position)
                parameters.append(parameter)
            if header_position + 2 != len(payload):
                raise ValueError("DefineFunction header has malformed code length")
            body_length = le16(payload, header_position)
            body = data[position : position + body_length]
            if len(body) != body_length:
                raise ValueError("truncated DefineFunction body")
            position += body_length
            action.update(
                name=name,
                parameters=parameters,
                body_length=body_length,
                body=parse_actions(body),
            )
        actions.append(action)
    raise ValueError("AVM1 action stream has no End action")


def parse_tags(data: bytes) -> tuple[list[int], dict[int, list[bytes]]]:
    if data[:3] != b"FWS" or data[3] != 9:
        raise ValueError("expected uncompressed SWF version 9")
    if le32(data, 4) != len(data):
        raise ValueError("FileLength does not match file size")
    bounds, rect_length = read_rect(data[8:])
    if bounds != [0, 640, 0, 640]:
        raise ValueError(f"expected 32x32 stage bounds, got {bounds}")
    frame_offset = 8 + rect_length
    if data[frame_offset : frame_offset + 4] != bytes((0x00, 0x0C, 0x01, 0x00)):
        raise ValueError("unexpected frame rate or frame count")
    position = frame_offset + 4
    codes: list[int] = []
    payloads: dict[int, list[bytes]] = {}
    while position + 2 <= len(data):
        header = le16(data, position)
        position += 2
        code, length = header >> 6, header & 0x3F
        if length == 0x3F:
            if position + 4 > len(data):
                raise ValueError("truncated long tag length")
            length = le32(data, position)
            position += 4
        payload = data[position : position + length]
        if len(payload) != length:
            raise ValueError("truncated SWF tag")
        position += length
        codes.append(code)
        payloads.setdefault(code, []).append(payload)
        if code == 0:
            if length != 0 or position != len(data):
                raise ValueError("End tag is not terminal")
            break
    else:
        raise ValueError("SWF has no End tag")
    return codes, payloads


def validate_shape(payload: bytes) -> None:
    if le16(payload, 0) != 1:
        raise ValueError("DefineShape character id is not 1")
    bounds, rect_length = read_rect(payload[2:])
    if bounds != [0, 640, 0, 640]:
        raise ValueError("shape bounds do not cover the stage")
    style = 2 + rect_length
    if payload[style : style + 6] != bytes((1, 0, 0x48, 0x90, 0xD8, 0)):
        raise ValueError("shape does not contain the expected solid fill")
    reader = BitReader(payload[style + 6 :])
    if (reader.read(4), reader.read(4)) != (1, 0):
        raise ValueError("unexpected shape fill/line bit counts")
    if [reader.read(1) for _ in range(6)] != [0, 0, 0, 1, 0, 1]:
        raise ValueError("shape style-change record is malformed")
    move_bits = reader.read(5)
    if reader.signed(move_bits) != 0 or reader.signed(move_bits) != 0 or reader.read(1) != 1:
        raise ValueError("shape move/fill record is malformed")
    for vertical, delta in ((0, 640), (1, 640), (0, -640), (1, -640)):
        if reader.read(1) != 1 or reader.read(1) != 1 or reader.read(4) + 2 != 11:
            raise ValueError("shape edge header is malformed")
        if reader.read(1) != 0 or reader.read(1) != vertical or reader.signed(11) != delta:
            raise ValueError("shape edge direction/delta is malformed")
    if reader.read(6) != 0:
        raise ValueError("shape is missing EndShapeRecord")


def validate_placement(payload: bytes) -> None:
    if payload[:5] != bytes((0x06, 0x01, 0x00, 0x01, 0x00)):
        raise ValueError("PlaceObject2 does not bind shape 1 at depth 1")
    reader = BitReader(payload[5:])
    if reader.read(1) != 0 or reader.read(1) != 0:
        raise ValueError("placement unexpectedly scales or rotates the shape")
    width = reader.read(5)
    if reader.signed(width) != 0 or reader.signed(width) != 0:
        raise ValueError("placement is not at the origin")


def validate_actions(payload: bytes) -> None:
    actions = parse_actions(payload)
    if actions[-1]["opcode"] != 0 or actions[-2]["opcode"] != 0x07:
        raise ValueError("frame does not stop before End")
    functions = [action for action in actions if action["opcode"] == 0x9B]
    if len(functions) != 2:
        raise ValueError("expected callback probe and host wrapper DefineFunction actions")
    expected_parameters = ["message_utf8", "score_number", "payload_object"]
    if any(action["opcode"] == 0x4F for action in actions):
        raise ValueError("named root methods must not be installed with SetMember")
    for function in functions:
        if not isinstance(function["name"], str) or not function["name"]:
            raise ValueError("root callback methods must use named DefineFunction")
        if function["parameters"] != expected_parameters:
            raise ValueError(f"callback function parameters are wrong: {function['parameters']!r}")
    expected_method_names = {"dirplayerCallbackProbe", "dirplayerInvokeCallbackProbe"}
    functions_by_name = {function["name"]: function for function in functions}
    if set(functions_by_name) != expected_method_names:
        raise ValueError(f"unexpected named callback methods: {set(functions_by_name)!r}")
    probe = functions_by_name["dirplayerCallbackProbe"]
    wrapper = functions_by_name["dirplayerInvokeCallbackProbe"]

    wrapper_index = next(index for index, action in enumerate(actions) if action is wrapper)
    expected_wrapper_type_actions = [
        (0x96, [("string", "_root.callbackWrapperType")]),
        (0x96, [("string", "_root.dirplayerInvokeCallbackProbe")]),
        (0x1C, None),
        (0x44, None),
        (0x1D, None),
    ]
    actual_wrapper_type_actions = [
        (int(action["opcode"]), action.get("values"))
        for action in actions[wrapper_index + 1 : wrapper_index + 1 + len(expected_wrapper_type_actions)]
    ]
    if actual_wrapper_type_actions != expected_wrapper_type_actions:
        raise ValueError(
            "root wrapper type probe is not the exact GetVariable/TypeOf/SetVariable sequence: "
            f"{actual_wrapper_type_actions!r}"
        )

    wrapper_counter_init = [
        (0x96, [("string", "_root.callbackWrapperInvocationCount")]),
        (0x96, [("int", 0)]),
        (0x1D, None),
    ]
    if not any(
        [
            (int(action["opcode"]), action.get("values"))
            for action in actions[index : index + len(wrapper_counter_init)]
        ]
        == wrapper_counter_init
        for index in range(len(actions) - len(wrapper_counter_init) + 1)
    ):
        raise ValueError("wrapper invocation counter is not initialized to zero")

    expected_probe_body: list[tuple[int, object]] = []
    for destination, parameter in (
        ("_root.callbackProbeString", "message_utf8"),
        ("_root.callbackProbeNumber", "score_number"),
        ("_root.callbackProbeObject", "payload_object"),
    ):
        expected_probe_body.extend(
            [(0x96, [("string", destination)]), (0x96, [("string", parameter)]), (0x1C, None), (0x1D, None)]
        )
    expected_probe_body.extend(
        [
            (0x96, [("string", "_root.callbackProbeInvocationCount")]),
            (0x96, [("string", "_root.callbackProbeInvocationCount")]),
            (0x1C, None),
            (0x96, [("int", 1)]),
            (0x0A, None),
            (0x1D, None),
            (0x96, [("string", "_root.callbackProbeArgumentCount")]),
            (0x96, [("int", 3)]),
            (0x1D, None),
            (0x96, [("string", "_root.callbackProbeDone")]),
            (0x96, [("int", 1)]),
            (0x1D, None),
            (0x00, None),
        ]
    )
    actual_probe_body = [(int(action["opcode"]), action.get("values")) for action in probe["body"]]
    if actual_probe_body != expected_probe_body:
        raise ValueError(f"callback probe body is malformed: {actual_probe_body!r}")

    expected_wrapper_body: list[tuple[int, object]] = []
    expected_wrapper_body.extend(
        [
            (0x96, [("string", "_root.callbackWrapperInvocationCount")]),
            (0x96, [("string", "_root.callbackWrapperInvocationCount")]),
            (0x1C, None),
            (0x96, [("int", 1)]),
            (0x0A, None),
            (0x1D, None),
        ]
    )
    expected_wrapper_body.extend([(0x96, [("string", "_root")]), (0x1C, None)])
    for parameter in ("score_number", "message_utf8"):
        expected_wrapper_body.extend([(0x96, [("string", parameter)]), (0x1C, None)])
    expected_wrapper_body.extend(
        [
            (0x96, [("int", 3)]),
            (0x96, [("string", "_root")]),
            (0x1C, None),
            (0x96, [("string", "dirplayerCallbackProbe")]),
            (0x52, None),
            (0x17, None),
            (0x00, None),
        ]
    )
    actual_wrapper_body = [(int(action["opcode"]), action.get("values")) for action in wrapper["body"]]
    if actual_wrapper_body != expected_wrapper_body:
        raise ValueError(f"host wrapper does not call the registered method: {actual_wrapper_body!r}")

    literals = [
        value
        for action in actions
        if action["opcode"] == 0x96
        for kind, value in action["values"]
        if kind == "string"
    ]
    if "café" not in literals or "object" not in literals:
        raise ValueError("UTF-8 or root object sentinel is missing from the authored action stream")
    if not any(
        actions[index].get("values") == [("string", "_root.kind")]
        and actions[index + 1].get("values") == [("string", "object")]
        and actions[index + 2]["opcode"] == 0x1D
        for index in range(len(actions) - 2)
    ):
        raise ValueError("_root.kind is not authored as the object sentinel")


def main() -> None:
    data = INPUT.read_bytes()
    codes, payloads = parse_tags(data)
    if codes != [9, 2, 26, 12, 1, 0]:
        raise ValueError(f"unexpected tag sequence: {codes}")
    if len(payloads[9]) != 1 or payloads[9][0] != bytes((0x18, 0x18, 0x18)):
        raise ValueError("background tag is malformed")
    validate_shape(payloads[2][0])
    validate_placement(payloads[26][0])
    validate_actions(payloads[12][0])
    digest = hashlib.sha256(data).hexdigest()
    print(f"{INPUT.name} ok bytes={len(data)} sha256={digest}")


if __name__ == "__main__":
    main()
