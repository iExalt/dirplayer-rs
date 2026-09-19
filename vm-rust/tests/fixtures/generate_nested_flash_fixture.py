#!/usr/bin/env python3
"""Generate tiny deterministic Director containers for the browser nested test.

The container is intentionally limited to the records consumed by the current
parser: DRCF, KEY*, CAS*, CASt, one raw SWF child, and one D5 VWSC frame.  The
SWF is authored from the checked-in 43-byte ActionScript fixture: its value,
background, and visible owner-colored shape are changed per owner.  No external
movie assets are read.
"""

from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parent
OUT = ROOT / "fixtures"
if (ROOT / "flash_initial_access.swf").exists():
    OUT = ROOT


def u16(value: int) -> bytes:
    return struct.pack(">H", value)


def le16(value: int) -> bytes:
    return struct.pack("<H", value)


def u32(value: int) -> bytes:
    return struct.pack(">I", value)


def fourcc(value: str) -> bytes:
    encoded = value.encode("ascii")
    assert len(encoded) == 4
    return encoded


class BitWriter:
    def __init__(self) -> None:
        self.bits: list[int] = []

    def write(self, value: int, width: int, *, signed: bool = False) -> None:
        if signed and value < 0:
            value = (1 << width) + value
        assert 0 <= value < (1 << width)
        self.bits.extend((value >> index) & 1 for index in range(width - 1, -1, -1))

    def finish(self) -> bytes:
        while len(self.bits) % 8:
            self.bits.append(0)
        result = bytearray()
        for offset in range(0, len(self.bits), 8):
            value = 0
            for bit in self.bits[offset:offset + 8]:
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


def visible_shape(rgb: tuple[int, int, int]) -> bytes:
    """Return a 32x32 twip rectangle with one owner-colored solid fill."""
    writer = BitWriter()
    writer.write(1, 4)  # one fill bit
    writer.write(0, 4)  # no line bits
    writer.write(0, 1)  # edge record flag
    writer.write(0, 1)  # no new styles
    writer.write(0, 1)  # no line style
    writer.write(1, 1)  # fill style 1
    writer.write(0, 1)  # fill style 0
    writer.write(1, 1)  # move-to present
    writer.write(1, 5)  # move bits
    writer.write(0, 1, signed=True)
    writer.write(0, 1, signed=True)
    writer.write(1, 1)  # fill style 1
    for vertical, delta in ((False, 640), (True, 640), (False, -640), (True, -640)):
        writer.write(1, 1)  # edge record
        writer.write(1, 1)  # straight edge
        writer.write(9, 4)  # 11-bit deltas, encoded as nbits - 2
        writer.write(0, 1)  # GeneralLineFlag: use a horizontal/vertical edge
        writer.write(1 if vertical else 0, 1)
        writer.write(delta, 11, signed=True)
    writer.write(0, 6)  # EndShapeRecord
    shape_records = writer.finish()
    shape = le16(1) + rect_bits(0, 0, 640, 640)
    shape += bytes((1, 0, rgb[0], rgb[1], rgb[2], 0))  # fill count/style, no lines
    shape += shape_records
    return chunk_payload(2, shape)


def placed_shape() -> bytes:
    payload = bytes((0x06,)) + le16(1) + le16(1) + matrix_bits(0, 0)
    return chunk_payload(26, payload)


def chunk_payload(tag_code: int, payload: bytes) -> bytes:
    assert 0 <= tag_code < 0x3F and len(payload) < 0x3F
    return struct.pack("<H", (tag_code << 6) | len(payload)) + payload


def chunk(tag: str, payload: bytes) -> bytes:
    return fourcc(tag) + u32(len(payload)) + payload


def authored_swf(value: int, rgb: tuple[int, int, int]) -> bytes:
    source_path = ROOT / "fixtures" / "flash_initial_access.swf"
    if not source_path.exists():
        source_path = ROOT / "flash_initial_access.swf"
    source = source_path.read_bytes()
    assert source[:3] == b"FWS" and source[8:14] == bytes.fromhex("50000a0000a0")
    # Keep the existing tiny DoAction body. The existing Push action stores the
    # integer at payload byte 12.
    action_start = source.index(bytes.fromhex("1303"))
    # The source tag is exactly 2 bytes of tag header plus a 19-byte DoAction
    # body. Keep the source's ActionScript only; the replacement stream below
    # owns the single ShowFrame and terminal End tags.
    action_tag = source[action_start:action_start + 21]
    action = bytearray(action_tag)
    assert action[:4] == bytes.fromhex("1303960e")
    assert action[4:16] == bytes.fromhex("000066697874757265000707")
    action[15] = value
    background = bytes.fromhex("4302") + bytes(rgb)
    show_frame = bytes.fromhex("4000")
    end = bytes.fromhex("0000")
    body = (
        rect_bits(0, 0, 640, 640)
        + source[14:18]
        + background
        + visible_shape(rgb)
        + placed_shape()
        + bytes(action)
        + show_frame
        + end
    )
    return source[:4] + struct.pack("<I", len(body) + 8) + body


def flash_info(rgb: tuple[int, int, int]) -> bytes:
    fixed = [0] * 26
    fixed[5] = 1  # centerRegPoint
    fixed[10] = 32  # bottom
    fixed[11] = 32  # right
    fixed[13] = (rgb[0] << 16) | (rgb[1] << 8) | rgb[2]
    fixed[14] = 1  # image enabled
    fixed[17] = 1  # loop enabled
    fixed[20] = 1  # no-scale
    fixed[23] = 0x3F800000  # scale 1.0
    variable = [0, 0, 0, 0x3F800000, 0, 0, 0, 1, 3, 0, 1, 1, 0, 1, 1, 0, 1]
    payload = u32(5) + b"flash" + u32(0) + b"FLSH" + u32(0) + u32(0)
    payload += b"".join(u32(v) for v in fixed + variable + [0, 0, 0, 0])
    payload += u32(0) + u32(2) + u32(0)
    return payload


def config() -> bytes:
    payload = bytearray(68)
    struct.pack_into(">HHHHHHHH", payload, 0, 68, 1201, 0, 0, 32, 32, 1, 1)
    payload[16:18] = b"\0\0"
    struct.pack_into(">HHHHH", payload, 18, 0, 0, 12, 0, 0)
    struct.pack_into(">H", payload, 28, 32)
    payload[30:32] = b"\0\0"
    struct.pack_into(">I", payload, 32, 0)
    struct.pack_into(">HH", payload, 36, 1201, 0)
    struct.pack_into(">III", payload, 40, 0, 0, 0)
    payload[52:54] = b"\0\0"
    struct.pack_into(">HHH", payload, 54, 30, 0, 0)
    struct.pack_into(">II", payload, 60, 0, 0)
    return bytes(payload)


def key_table() -> bytes:
    entries = [
        (2, 2, "CAS*"),
        (3, 2, "CASt"),
        (4, 3, "SWF "),
        (5, 0, "VWSC"),
    ]
    payload = u16(12) + u16(12) + u32(len(entries)) + u32(len(entries))
    return payload + b"".join(u32(section) + u32(owner) + fourcc(tag) for section, owner, tag in entries)


def score() -> bytes:
    frame = bytearray(96)
    # D5 main channels occupy 48 bytes.  The first sprite record follows.
    sprite = bytearray(24)
    sprite[0] = 1
    sprite[2:4] = u16(1)
    sprite[4:6] = u16(1)
    sprite[12:14] = u16(16)
    sprite[14:16] = u16(16)
    sprite[16:18] = u16(32)
    sprite[18:20] = u16(32)
    frame[48:72] = sprite
    delta = u16(96) + u16(0) + bytes(frame)
    stream = u16(len(delta) + 2) + delta + u16(0)
    header = u32(len(stream) + 20) + u32(20) + u32(1) + u16(7) + u16(24) + u16(4) + u16(48)
    return header + stream


def director_container(swf: bytes) -> bytes:
    payloads = [config(), key_table(), u32(3), u32(8) + u32(0) + u32(len(flash_info((0, 0, 0)))) + flash_info((0, 0, 0)), swf, score()]
    chunks = [chunk(tag, payload) for tag, payload in zip(("DRCF", "KEY*", "CAS*", "CASt", "SWF ", "VWSC"), payloads)]
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
    for index, (item, offset) in enumerate(zip(chunks, offsets)):
        data += fourcc(item[:4].decode("ascii")) + u32(len(item) - 8) + u32(offset) + u16(0) + u16(0) + u32(0)
    return bytes(data)


def write_fixture(name: str, value: int, rgb: tuple[int, int, int]) -> None:
    swf = authored_swf(value, rgb)
    # CASt's FlashInfo and the SWF child are independent chunks; rebuild the
    # container with the owner-specific metadata rather than mutating bytes.
    payloads = [config(), key_table(), u32(3), u32(8) + u32(0) + u32(len(flash_info(rgb))) + flash_info(rgb), swf, score()]
    tags = ("DRCF", "KEY*", "CAS*", "CASt", "SWF ", "VWSC")
    chunks = [chunk(tag, payload) for tag, payload in zip(tags, payloads)]
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
    (OUT / f"nested_flash_{name}.swf").write_bytes(swf)
    (OUT / f"nested_flash_{name}.dcr").write_bytes(data)


def write_bad_startup_fixture() -> None:
    """Keep a valid Director envelope but corrupt only its embedded SWF.

    This is a deterministic malformed-SWF input for production startup tests;
    lifecycle rollback is exercised separately through the real registrar
    failure hook, rather than inferred from this structural fixture.
    """
    data = bytearray((OUT / "nested_flash_a.dcr").read_bytes())
    mmap_offset = int.from_bytes(data[24:28], "big")
    swf_entry = mmap_offset + 32 + 4 * 20
    swf_offset = int.from_bytes(data[swf_entry + 8:swf_entry + 12], "big") + 8
    data[swf_offset:swf_offset + 48] = b"BAD!" + bytes(44)
    (OUT / "nested_flash_bad.dcr").write_bytes(data)


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    write_fixture("a", 7, (0xC0, 0x20, 0x20))
    write_fixture("b", 9, (0x20, 0x40, 0xC0))
    write_bad_startup_fixture()
