#!/usr/bin/env python3
"""Decode msi-ec ec_dump snapshots taken by ec-dump.sh (phase 2 spike, read-only).

    scripts/spike/ec-decode.py <out-dir> [<out-dir> ...]

Prints the fan-curve tables once per directory and one line per sample with the
realtime values, so changes over time and between modes are easy to spot.
"""
import pathlib
import sys

# Provisional layout, see docs/hardware/ec-map.md.
CPU = {"temp": 0x68, "fan": 0x71, "up": range(0x6A, 0x70), "speeds": range(0x72, 0x79), "down": range(0x7A, 0x80)}
GPU = {"temp": 0x80, "fan": 0x89, "up": range(0x82, 0x88), "speeds": range(0x8A, 0x91), "down": range(0x92, 0x98)}
MODES = {"shift(0xd2)": 0xD2, "fan(0xd4)": 0xD4, "boost(0x98)": 0x98, "charge(0xd7)": 0xD7}


def parse_dump(path: pathlib.Path) -> bytes:
    data = bytearray()
    for line in path.read_text().splitlines():
        if line.startswith("| 0x"):
            data += bytes(int(b, 16) for b in line.split("|")[2].split())
    if len(data) != 256:
        raise ValueError(f"{path}: expected 256 bytes, got {len(data)}")
    return bytes(data)


def table(ec: bytes, fan: dict) -> str:
    up = [ec[a] for a in fan["up"]]
    speeds = [ec[a] for a in fan["speeds"]]
    down = [ec[a] for a in fan["down"]]
    return f"speeds%={speeds} up°C={up} down°C={down}"


def main() -> None:
    for d in map(pathlib.Path, sys.argv[1:]):
        dumps = sorted(d.glob("dump-*.txt"))
        first = parse_dump(dumps[0])
        print(f"== {d.name}  fw={first[0xA0:0xAC].decode(errors='replace')}")
        print("   CPU", table(first, CPU))
        print("   GPU", table(first, GPU))
        print("   " + " ".join(f"{k}={first[v]:#04x}" for k, v in MODES.items()))
        prev = None
        for p in dumps:
            ec = parse_dump(p)
            changed = [] if prev is None else [f"{i:#04x}:{prev[i]:02x}>{ec[i]:02x}" for i in range(256)
                                               if prev[i] != ec[i] and i not in (CPU["temp"], GPU["temp"], CPU["fan"], GPU["fan"])]
            print(f"   {p.stem}: cpu {ec[CPU['temp']]}°C {ec[CPU['fan']]}%  gpu {ec[GPU['temp']]}°C {ec[GPU['fan']]}%"
                  + (f"  other: {' '.join(changed[:12])}" if changed else ""))
            prev = ec


if __name__ == "__main__":
    main()
