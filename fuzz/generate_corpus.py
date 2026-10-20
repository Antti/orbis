#!/usr/bin/env python3
"""Generate small, redistribution-safe seeds without using the parsers under test."""
import hashlib
import hmac
from pathlib import Path
import struct
import zlib

ROOT = Path(__file__).resolve().parent / "corpus"
BS = 4096


def put(data, offset, fmt, value):
    struct.pack_into(fmt, data, offset, value)


def seed(target, name, data):
    folder = ROOT / target
    folder.mkdir(parents=True, exist_ok=True)
    (folder / name).write_bytes(data)


def sfo():
    keys = b"TITLE\0SYSTEM_VER\0BLOB\0"
    values = b"Fuzz\0" + struct.pack("<I", 0x05050000) + b"\0\xff"
    data = bytearray(20 + 3 * 16)
    data[:4] = b"\0PSF"
    struct.pack_into("<4I", data, 4, 0x101, len(data), len(data) + len(keys), 3)
    for i, (key, kind, size, offset) in enumerate([(0, 0x204, 5, 0), (6, 0x404, 4, 5), (17, 4, 2, 9)]):
        struct.pack_into("<HHIII", data, 20 + i * 16, key, kind, size, size, offset)
    return data + keys + values


def pkg(name):
    data = bytearray(0x1100)
    data[:4] = b"\x7fCNT"
    data[0x40:0x64] = b"UP0000-CUSA00000_00-0000000000000000"
    put(data, 0x10, ">I", 3)
    put(data, 0x18, ">I", 0x1000)
    for i, (entry_id, payload) in enumerate([(0x200, name + b"\0"), (0xdeadbeef, b"test"), (0x1000, sfo())]):
        struct.pack_into(">IIIIIIII", data, 0x1000 + i * 32, entry_id, 0, 0, 0, len(data), len(payload), 0, 0)
        data.extend(payload)
    return data


def pfs(signed=False, count=1, table_blocks=1):
    # One directory inode. Nonzero type-SELF entries exercise the directory
    # reader without manufacturing a recursively growing traversal.
    indirect = int(count > 12)
    table_indirect = int(table_blocks > 12)
    start = table_blocks + 1
    data = bytearray((start + count + indirect + table_indirect) * BS)
    put(data, 0, "<Q", 1)
    put(data, 8, "<Q", 20130315)
    put(data, 0x1c, "<H", int(signed))
    put(data, 0x20, "<I", BS)
    put(data, 0x30, "<Q", 1)
    put(data, 0x40, "<Q", table_blocks)
    put(data, BS, "<H", 0x4000)
    put(data, BS + 8, "<Q", count * BS)
    put(data, BS + 16, "<Q", count * BS)
    put(data, BS + 96, "<I", count)
    key = hmac.digest(b"\x43" * 32, b"\x02\0\0\0" + bytes(16), "sha256")

    def pointer(at, block, width=4):
        if signed:
            data[at:at+32] = hmac.digest(key, data[block*BS:(block+1)*BS], "sha256")
            at += 32
        put(data, at, "<Q" if width == 8 else "<I", block)

    for i in range(count):
        struct.pack_into("<4I", data, (start + i) * BS, 0, 4, 1, 17)
        data[(start + i) * BS + 16] = ord('.')
    stride = 36 if signed else 4
    for i in range(min(count, 12)):
        pointer(BS + 100 + i * stride, start + i)
    if indirect:
        for i in range(count - 12):
            pointer((start + count) * BS + i * stride, start + 12 + i)
        pointer(BS + 100 + 12 * stride, start + count)
    if signed:
        for i in range(min(table_blocks, 12)):
            pointer(0xb8 + i * 40, i + 1, 8)
        if table_indirect:
            block = start + count + indirect
            for i in range(table_blocks - 12):
                pointer(block * BS + i * 40, 13 + i, 8)
            pointer(0xb8 + 12 * 40, block, 8)
        data[0x380:0x3a0] = hmac.digest(key, data[:0x5a0], "sha256")
    return data


def pfsc(payload, kind="compressed"):
    blocks = [payload[i:i+BS] for i in range(0, len(payload), BS)]
    data = bytearray(48 + (len(blocks) + 1) * 8)
    data[:4] = b"PFSC"
    put(data, 12, "<I", BS)
    put(data, 16, "<Q", BS)
    put(data, 24, "<Q", 48)
    put(data, 40, "<Q", len(payload))
    for i, block in enumerate(blocks):
        put(data, 48 + i * 8, "<Q", len(data))
        data.extend(zlib.compress(block) if kind == "compressed" else block.ljust(BS, b"\0"))
    put(data, 48 + len(blocks) * 8, "<Q", len(data))
    return data



def compressed_file():
    logical = b"A" * (BS + 17)
    container = pfsc(logical)
    blocks = (len(container) + BS - 1) // BS
    data = pfs()
    data.extend(bytes(blocks * BS))
    put(data, 0x30, "<Q", 2)
    struct.pack_into("<4I", data, 2 * BS, 1, 2, 4, 20)
    data[2 * BS + 16:2 * BS + 20] = b"data"
    at = BS + 168
    put(data, at, "<H", 0x8000)
    put(data, at + 4, "<I", 1)
    put(data, at + 8, "<Q", len(container))
    put(data, at + 16, "<Q", len(logical))
    put(data, at + 96, "<I", blocks)
    put(data, at + 100, "<I", 3)
    put(data, at + 104, "<I", 0xffffffff)
    data[3 * BS:3 * BS + len(container)] = container
    return data


def main():
    seed("sfo", "typed", sfo())
    for name in [b"custom.dat", b"nested/item.dat", b"../escape", b"CON", b"x:y", b"\xff"]:
        seed("pkg", "name-" + name.hex(), pkg(name))
    seed("pfs", "unsigned", pfs())
    seed("pfs", "compressed-file", compressed_file())
    seed("pfs", "signed", pfs(True))
    seed("pfs", "indirect-data", pfs(True, count=13))
    seed("pfs", "indirect-table", pfs(True, table_blocks=13))
    seed("pfsc", "compressed", pfsc(pfs()))
    seed("pfsc", "stored", pfsc(pfs(), "stored"))
    seed("pfsc", "partial-final-block", pfsc(b"hello"))
    sparse = pfsc(bytes(BS), "stored")
    put(sparse, 56, "<Q", len(sparse) + 1)
    seed("pfsc", "sparse", sparse[:64])
    for target in ["pkg", "sfo", "pfs", "pfsc"]:
        seed(target, "empty", b"")
        seed(target, "truncated", b"\xff" * 31)


if __name__ == "__main__":
    main()
