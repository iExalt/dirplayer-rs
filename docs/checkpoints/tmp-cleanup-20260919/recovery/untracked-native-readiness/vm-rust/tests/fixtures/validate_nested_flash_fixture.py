#!/usr/bin/env python3
"""Structural validation for the staged bytes before Rust parser integration."""

from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parent / "fixtures"
if not ROOT.exists():
    ROOT = Path(__file__).resolve().parent


def be16(data, offset):
    return struct.unpack_from(">H", data, offset)[0]


def be32(data, offset):
    return struct.unpack_from(">I", data, offset)[0]


def le16(data, offset):
    return struct.unpack_from("<H", data, offset)[0]


class BitReader:
    def __init__(self, data):
        self.data = data
        self.offset = 0

    def read(self, width):
        value = 0
        for _ in range(width):
            byte = self.data[self.offset // 8]
            value = (value << 1) | ((byte >> (7 - self.offset % 8)) & 1)
            self.offset += 1
        return value

    def signed(self, width):
        value = self.read(width)
        if value & (1 << (width - 1)):
            value -= 1 << width
        return value

    def align(self):
        self.offset = (self.offset + 7) & ~7


def read_rect(data):
    reader = BitReader(data)
    width = reader.read(5)
    values = [reader.signed(width) for _ in range(4)]
    reader.align()
    return values, reader.offset // 8


def validate_visible_shape(payload, expected_rgb):
    assert le16(payload, 0) == 1
    bounds, offset = read_rect(payload[2:])
    assert bounds == [0, 640, 0, 640]
    style_offset = 2 + offset
    assert payload[style_offset] == 1  # one fill style
    assert payload[style_offset + 1] == 0  # solid fill
    assert tuple(payload[style_offset + 2:style_offset + 5]) == expected_rgb
    assert payload[style_offset + 5] == 0  # no line styles
    reader = BitReader(payload[style_offset + 6:])
    assert reader.read(4) == 1  # NumFillBits
    assert reader.read(4) == 0  # NumLineBits
    assert reader.read(1) == 0  # style-change record
    assert reader.read(1) == 0  # no new styles
    assert reader.read(1) == 0  # no line style
    assert reader.read(1) == 1  # fill style 1
    assert reader.read(1) == 0  # fill style 0
    assert reader.read(1) == 1  # move-to present
    move_bits = reader.read(5)
    assert reader.signed(move_bits) == 0 and reader.signed(move_bits) == 0
    assert reader.read(1) == 1  # fill style 1
    expected_edges = [(False, 640), (True, 640), (False, -640), (True, -640)]
    for expected_vertical, expected_delta in expected_edges:
        assert reader.read(1) == 1  # edge record
        assert reader.read(1) == 1  # straight edge
        assert reader.read(4) + 2 == 11
        assert reader.read(1) == 0  # GeneralLineFlag
        assert reader.read(1) == int(expected_vertical)
        assert reader.signed(11) == expected_delta
    assert reader.read(6) == 0  # EndShapeRecord


def validate_shape_placement(payload):
    assert payload[0] == 0x06  # HAS_CHARACTER + HAS_MATRIX
    assert le16(payload, 1) == 1
    assert le16(payload, 3) == 1
    reader = BitReader(payload[5:])
    assert reader.read(1) == 0  # HasScale
    assert reader.read(1) == 0  # HasRotate
    width = reader.read(5)
    assert reader.signed(width) == 0
    assert reader.signed(width) == 0


def validate(path):
    data = path.read_bytes()
    assert data[:4] == b"RIFX" and data[8:12] == b"MV93"
    assert be32(data, 4) == len(data) - 8
    assert data[12:16] == b"imap"
    mmap = be32(data, 24)
    assert data[mmap:mmap + 4] == b"mmap"
    used = be32(data, mmap + 16)
    entries = []
    for index in range(used):
        entry = mmap + 32 + index * 20
        tag = data[entry:entry + 4]
        size = be32(data, entry + 4)
        offset = be32(data, entry + 8)
        assert data[offset:offset + 4] == tag
        assert be32(data, offset + 4) == size
        entries.append((tag, data[offset + 8:offset + 8 + size]))
    assert [tag for tag, _ in entries] == [b"DRCF", b"KEY*", b"CAS*", b"CASt", b"SWF ", b"VWSC"]
    assert be16(entries[0][1], 0) == 68 and be16(entries[0][1], 36) == 1201
    keys = entries[1][1]
    assert be16(keys, 0) == 12 and be32(keys, 4) == 4 and be32(keys, 8) == 4
    assert be32(entries[2][1], 0) == 3
    cast = entries[3][1]
    assert be32(cast, 0) == 8 and be32(cast, 4) == 0
    specific_len = be32(cast, 8)
    specific = cast[12:12 + specific_len]
    assert be32(specific, 0) == 5 and specific[4:9] == b"flash"
    swf = entries[4][1]
    if path.stem.endswith("_bad"):
        assert swf[:4] == b"BAD!"
        print(path.name, "ok", len(data), "bytes (malformed embedded SWF fixture)")
        return
    assert swf[:3] == b"FWS" and struct.unpack_from("<I", swf, 4)[0] == len(swf)
    rect_bits = swf[8] >> 3
    rect_len = (5 + rect_bits * 4 + 7) // 8
    stage_bounds, _ = read_rect(swf[8:8 + rect_len])
    assert stage_bounds == [0, 640, 0, 640]
    frame_count = struct.unpack_from("<H", swf, 8 + rect_len + 2)[0]
    tag_pos = 8 + rect_len + 4
    show_frames = 0
    swf_tags = []
    while tag_pos + 2 <= len(swf):
        header = struct.unpack_from("<H", swf, tag_pos)[0]
        tag_pos += 2
        tag_code, tag_len = header >> 6, header & 0x3F
        if tag_len == 0x3F:
            assert tag_pos + 4 <= len(swf)
            tag_len = struct.unpack_from("<I", swf, tag_pos)[0]
            tag_pos += 4
        assert tag_pos + tag_len <= len(swf)
        swf_tags.append((tag_code, swf[tag_pos:tag_pos + tag_len]))
        if tag_code == 1:
            show_frames += 1
        if tag_code == 0:
            assert tag_len == 0 and tag_pos == len(swf)
            break
        tag_pos += tag_len
    assert show_frames == frame_count == 1
    expected = {
        "a": (7, (0xC0, 0x20, 0x20)),
        "b": (9, (0x20, 0x40, 0xC0)),
    }[path.stem.rsplit("_", 1)[-1]]
    value, expected_rgb = expected
    shapes = [payload for code, payload in swf_tags if code == 2]
    placements = [payload for code, payload in swf_tags if code == 26]
    actions = [payload for code, payload in swf_tags if code == 12]
    assert len(shapes) == 1 and len(placements) == 1 and len(actions) == 1
    validate_visible_shape(shapes[0], expected_rgb)
    validate_shape_placement(placements[0])
    assert b"fixture\x00" in actions[0]
    assert bytes((0x07, value)) in actions[0]
    assert entries[5][1][:20] and be32(entries[5][1], 8) == 1
    print(path.name, "ok", len(data), "bytes")


for fixture in sorted(ROOT.glob("nested_flash_*.dcr")):
    validate(fixture)
