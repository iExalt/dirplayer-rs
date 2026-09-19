#!/usr/bin/env python3
"""Generate the accepted deterministic D5 native readiness movie.

The fixture contains a QuickDraw shape, a three-frame score, and a movie script
with startup/frame and native-input handlers. It uses DRCF, KEY*, CAS*, CASt, VWSC, Lctx,
Lnam, and Lscr records and excludes external resource families.
"""

from pathlib import Path
import struct


ROOT = Path(__file__).resolve().parent
OUTPUT = ROOT / "native_director_probe.dcr"


def u16(value: int) -> bytes:
    return struct.pack(">H", value)


def i16(value: int) -> bytes:
    return struct.pack(">h", value)


def u32(value: int) -> bytes:
    return struct.pack(">I", value)


def fourcc(value: str) -> bytes:
    encoded = value.encode("ascii")
    assert len(encoded) == 4
    return encoded


def chunk(tag: str, payload: bytes) -> bytes:
    return fourcc(tag) + u32(len(payload)) + payload


def config() -> bytes:
    # DRCF version 1201, 32x32 stage, one cast library.
    payload = bytearray(68)
    struct.pack_into(">HHHHHHHH", payload, 0, 68, 1201, 0, 0, 32, 32, 1, 1)
    struct.pack_into(">HHHHH", payload, 18, 0, 0, 12, 0, 0)
    struct.pack_into(">H", payload, 28, 32)
    struct.pack_into(">I", payload, 32, 0)
    struct.pack_into(">HH", payload, 36, 1201, 0)
    struct.pack_into(">III", payload, 40, 0, 0, 0)
    struct.pack_into(">HHH", payload, 54, 30, 0, 0)
    return bytes(payload)


def key_table() -> bytes:
    entries = [
        (2, 2, "CAS*"),
        (3, 2, "CASt"),
        (4, 2, "CASt"),
        (5, 0, "VWSC"),
        (6, 1024, "Lctx"),
        (7, 6, "Lnam"),
        (8, 6, "Lscr"),
    ]
    header = u16(12) + u16(12) + u32(len(entries)) + u32(len(entries))
    return header + b"".join(
        u32(section) + u32(cast_id) + fourcc(tag)
        for section, cast_id, tag in entries
    )


def script_context() -> bytes:
    # One Lscr section is referenced by the Lctx map. The native cast loader
    # associates this context with the implicit cast id 1024.
    payload = bytearray(42 + 12)
    struct.pack_into(">IIIIHHIII IHHH", payload, 0,
                     0, 0, 1, 1, 42, 0, 0, 0, 0, 7, 1, 0, 0)
    struct.pack_into(">Iihh", payload, 42, 0, 8, 0, 0)
    return bytes(payload)


def script_names() -> bytes:
    names = (
        "prepareMovie", "startMovie", "enterFrame", "exitFrame",
        "prepareState", "startState", "enterState", "exitState", "frameState",
        "timerA", "timerB", "forget", "period", "timeoutObject",
        "mouseDown", "mouseH", "mouseV", "sprite", "locH", "inputH", "inputV",
    )
    encoded = b"".join(bytes((len(name),)) + name.encode("ascii") for name in names)
    return u32(0) + u32(0) + u32(len(encoded)) + u32(len(encoded)) + u16(20) + u16(len(names)) + encoded


def script_chunk() -> bytes:
    # ScriptChunk reads a fixed header beginning at payload offset 8. The
    # startup handlers write phase globals; enterFrame increments frameState.
    # mouseDown records the canonical mouseH/mouseV values and moves the shape
    # through the ordinary sprite property setter. timerA adds 10 and
    # reschedules itself to period 200; timerB multiplies frameState by 10 and
    # calls forget on its timeout argument.
    handler_count = 7
    header_size = 92
    record_size = 42
    handlers_offset = header_size
    compiled_offset = handlers_offset + handler_count * record_size
    bytecodes = []
    records = bytearray()
    for handler_index, handler_name_id in enumerate((0, 1, 2, 3, 14, 9, 10)):
        if handler_index == 0:
            # Initialize frameState after publishing prepareState.
            code = bytes((0x41, 1, 0x4F, 4, 0x41, 1, 0x4F, 8, 0x01))
        elif handler_index == 2:
            # frameState = frameState + 1 on every enterFrame.
            code = bytes((0x41, 1, 0x4F, 6, 0x49, 8, 0x41, 1, 0x05, 0x4F, 8, 0x01))
        elif handler_index < 4:
            code = bytes((0x41, 1, 0x4F, 4 + handler_index, 0x01))
        elif handler_index == 4:
            # inputH = the mouseH; inputV = the mouseV;
            # sprite(1).locH = the mouseH.
            code = bytes((
                0x5F, 15, 0x4F, 19,
                0x5F, 16, 0x4F, 20,
                0x41, 1, 0x43, 1, 0x57, 17, 0x5F, 15, 0x62, 18,
                0x01,
            ))
        elif handler_index == 5:
            # frameState += 10; timeoutObject.period = 200
            code = bytes((
                0x49, 8, 0x41, 10, 0x05, 0x4F, 8,
                0x4B, 0, 0x81, 0, 200, 0x62, 12, 0x01,
            ))
        else:
            # frameState *= 10; timeoutObject.forget()
            code = bytes((
                0x49, 8, 0x41, 10, 0x04, 0x4F, 8,
                0x4B, 0, 0x42, 1, 0x67, 11, 0x01,
            ))
        offset = compiled_offset + len(b"".join(bytecodes))
        argument_count = 1 if handler_index >= 5 else 0
        argument_offset = 0
        if argument_count:
            argument_offset = 0  # patched after the payload layout is known
        bytecodes.append(code)
        records += struct.pack(">HHIIH I H I H I IH H I", handler_name_id, 0, len(code), offset,
                               argument_count, argument_offset, 0, 0, 0, 0, 0, 0, 0, 0)
    globals_offset = compiled_offset + sum(len(code) for code in bytecodes)
    argument_offset = globals_offset + 14
    payload = bytearray(argument_offset + 2)
    struct.pack_into(">IIHHHH", payload, 8, len(payload), len(payload), 84, 0, 0, 0)
    struct.pack_into(">IHIHHIIHIHIHIHIII", payload, 38,
                     0, 0, 0, 0, 0, 0, 0, 0, 0,
                     7, globals_offset, handler_count, handlers_offset,
                     0, 0, 0, 0)
    payload[handlers_offset:handlers_offset + len(records)] = records
    cursor = compiled_offset
    for code in bytecodes:
        payload[cursor:cursor + len(code)] = code
        cursor += len(code)
    for index in range(7):
        struct.pack_into(">H", payload, globals_offset + index * 2, 4 + index if index < 5 else 19 + index - 5)
    struct.pack_into(">H", payload, argument_offset, 13)
    # The two timeout records each use the shared single argument table.
    for index in (5, 6):
        struct.pack_into(">I", payload, handlers_offset + index * record_size + 14, argument_offset)
    return bytes(payload)


def shape_member() -> bytes:
    # D5 CASt: raw type 8 is disambiguated as QuickDraw Shape by the 17-byte
    # ShapeInfo payload.  Rect is 0,0..32,32, with a solid foreground fill.
    shape_info = (
        u16(1)  # ShapeType::Rect
        + i16(0) + i16(0) + i16(32) + i16(32)
        + u16(1)  # pattern
        + bytes((12, 2, 1, 1, 0))  # fore, back, fill, line width, direction
    )
    assert len(shape_info) == 17
    return u32(8) + u32(0) + u32(len(shape_info)) + shape_info


def script_member() -> bytes:
    # The second CASt resource is listed first by CAS*, so it becomes movie
    # cast member 1 and points at Lscr script id 1. CastMemberInfo's minimal
    # BasicList carries four empty source/name/path entries.
    info = bytearray(46)
    struct.pack_into(">IIIII", info, 0, 20, 0, 0, 0, 1)
    struct.pack_into(">HIIII", info, 20, 4, 0, 1, 2, 3)
    struct.pack_into(">I", info, 38, 4)
    return u32(11) + u32(len(info)) + u32(2) + info + u16(3)


def score() -> bytes:
    frame = bytearray(96)
    sprite = bytearray(24)
    sprite[0] = 1  # sprite exists
    sprite[10] = 255  # foreground palette index (black in SYSTEM_WIN_PALETTE)
    sprite[11] = 0  # background palette index (white in SYSTEM_WIN_PALETTE)
    sprite[2:4] = u16(1)  # cast library
    sprite[4:6] = u16(2)  # cast member (shape; member 1 is the movie script)
    sprite[12:14] = u16(4)  # left
    sprite[14:16] = u16(4)  # top
    sprite[16:18] = u16(24)  # height; rendered rect is (4,4)-(28,28)
    sprite[18:20] = u16(24)  # width; rendered rect is (4,4)-(28,28)
    frame[48:72] = sprite

    # Preserve three identical valid full frame records. The final u16(0) remains
    # the stream terminator; it is not counted as a frame.
    delta = u16(96) + u16(0) + bytes(frame)
    frame_record = u16(len(delta) + 2) + delta
    stream = frame_record + frame_record + frame_record + u16(0)
    header = (
        u32(len(stream) + 20) + u32(20) + u32(3)
        + u16(7) + u16(24) + u16(4) + u16(48)
    )
    return header + stream


def container() -> bytes:
    chunks = [
        chunk("DRCF", config()),
        chunk("KEY*", key_table()),
        # Member 1 is the movie script and member 2 is the QuickDraw shape;
        # D5+ rendering reserves shape member 1 as a placeholder.
        chunk("CAS*", u32(4) + u32(3)),
        chunk("CASt", shape_member()),
        chunk("CASt", script_member()),
        chunk("VWSC", score()),
        chunk("Lctx", script_context()),
        chunk("Lnam", script_names()),
        chunk("Lscr", script_chunk()),
    ]
    data = bytearray(b"RIFX" + u32(0) + b"MV93")
    data += fourcc("imap") + u32(8) + u32(len(chunks))
    mmap_offset = 12 + 16 + sum(len(item) for item in chunks)
    data += u32(mmap_offset)
    offsets = []
    for item in chunks:
        offsets.append(len(data))
        data += item

    data += fourcc("mmap") + u32(24 + 20 * len(chunks))
    data += u16(24) + u16(20) + u32(len(chunks)) + u32(len(chunks))
    data += u32(0xFFFFFFFF) + u32(0xFFFFFFFF) + u32(0xFFFFFFFF)
    for item, offset in zip(chunks, offsets):
        data += item[:4] + u32(len(item) - 8) + u32(offset) + u16(0) + u16(0) + u32(0)
    data[4:8] = u32(len(data) - 8)
    return bytes(data)


def main() -> None:
    payload = container()
    OUTPUT.write_bytes(payload)
    print(f"wrote {OUTPUT} ({len(payload)} bytes)")


if __name__ == "__main__":
    main()
