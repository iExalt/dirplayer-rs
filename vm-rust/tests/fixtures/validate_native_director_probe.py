#!/usr/bin/env python3
"""Validate the deterministic native Director probe without the Rust parser."""

from pathlib import Path
import hashlib
import struct


ROOT = Path(__file__).resolve().parent
INPUT = ROOT / "native_director_probe.dcr"
FORBIDDEN = {b"SWF ", b"FLSH", b"Xtra", b"SND ", b"FNT ", b"MCsL"}


def u16(raw: bytes, offset: int) -> int:
    return struct.unpack_from(">H", raw, offset)[0]


def u32(raw: bytes, offset: int) -> int:
    return struct.unpack_from(">I", raw, offset)[0]


def main() -> None:
    raw = INPUT.read_bytes()
    assert raw[:4] == b"RIFX" and raw[8:12] == b"MV93"
    assert u32(raw, 4) == len(raw) - 8
    assert not any(tag in raw for tag in FORBIDDEN)

    assert raw[12:16] == b"imap"
    mmap_offset = u32(raw, 24)
    assert raw[mmap_offset:mmap_offset + 4] == b"mmap"
    count = u32(raw, mmap_offset + 12)
    assert count == 9

    chunks = {}
    cursor = 28
    while cursor < mmap_offset:
        tag = raw[cursor:cursor + 4]
        size = u32(raw, cursor + 4)
        payload = raw[cursor + 8:cursor + 8 + size]
        if tag in chunks:
            previous = chunks[tag]
            chunks[tag] = previous + [payload] if isinstance(previous, list) else [previous, payload]
        else:
            chunks[tag] = payload
        cursor += 8 + size
    assert set(chunks) == {b"DRCF", b"KEY*", b"CAS*", b"CASt", b"VWSC", b"Lctx", b"Lnam", b"Lscr"}

    assert u16(chunks[b"DRCF"], 0) == 68
    assert u16(chunks[b"DRCF"], 2) == 1201
    assert u16(chunks[b"DRCF"], 8) == 32
    assert u16(chunks[b"DRCF"], 10) == 32

    key = chunks[b"KEY*"]
    assert u16(key, 0) == 12 and u16(key, 2) == 12
    assert u32(key, 4) == u32(key, 8) == 7
    assert [key[offset + 8:offset + 12] for offset in (12, 24, 36, 48, 60, 72, 84)] == [b"CAS*", b"CASt", b"CASt", b"VWSC", b"Lctx", b"Lnam", b"Lscr"]

    assert chunks[b"CAS*"] == struct.pack(">II", 3, 4)
    cast = chunks[b"CASt"][0]
    assert u32(cast, 0) == 8 and u32(cast, 4) == 0 and u32(cast, 8) == 17
    assert u16(cast, 12) == 1
    assert struct.unpack_from(">hhhh", cast, 14) == (0, 0, 32, 32)

    score = chunks[b"VWSC"]
    assert u32(score, 8) == 1
    assert len(score) >= 20 + 2 + 2 + 2 + 96 + 2
    lctx = chunks[b"Lctx"]
    assert u32(lctx, 8) == 1 and u16(lctx, 16) == 42 and u32(lctx, 32) == 7
    assert struct.unpack_from(">i", lctx, 46)[0] == 8
    lnam = chunks[b"Lnam"]
    assert u16(lnam, 18) == 8
    lscr = chunks[b"Lscr"]
    assert u16(lscr, 66) == 4 and u16(lscr, 72) == 4 and u16(lscr, 78) == 0
    digest = hashlib.sha256(raw).hexdigest()
    print(f"valid {INPUT} bytes={len(raw)} sha256={digest}")


if __name__ == "__main__":
    main()
