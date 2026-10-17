#!/usr/bin/env python3
"""Cash hotkey for Story mode: each press of the key right of 0 (KeyboardEvent.code Minus) adds $999,999 to the current character's SPn_TOTAL_CASH.

The asset files stay untouched (the launcher checks their sizes, and a torrent recheck would re-download them).
The patched copies go to GrandV-assets/overrides/, which the launcher serves in place of the asset files.

- game.html: a Minus keydown sets a flag byte in the engine's keyboard block and is not passed to the engine.
- game.movie.wasm (or the variants given): the CStatsMgr::Update entry consumes the flag and calls the same
  GetStatId / IsKeyValid / IncrementStat path a money pickup uses.

Usage: python3 cash_hotkey.py /path/to/GrandV-assets [game.movie.wasm ...]
Run it again after each asset update; the launcher ignores an override that is older than its asset.
"""
import struct
import sys
from pathlib import Path

CASH_AMOUNT = 999999.0  # IncrementStat takes a float; exact below 2**24
CASH_KEY = 0xE8         # flag byte in the 256-byte keyboard block; VK 0xE8 is unassigned
STATS_UPDATE = 'CStatsMgr::Update()'
GET_STAT_ID = 'StatId_char::GetStatId() const'
IS_KEY_VALID = 'CStatsDataMgr::IsKeyValid(StatId const&) const'
INCREMENT_STAT = 'CStatsMgr::IncrementStat(StatId const&, float, unsigned int)'
STAT_ID_CTOR = 'StatId_char::StatId_char('
INPUT_TEST_KEY = 'wasm_input_test_key'
UPDATE_PROLOGUE = bytes.fromhex('230042407c22052400')  # sp-based frame setup at the original CStatsMgr::Update entry
HTML_ANCHOR = 'e.preventDefault();\n\t\tif(!pressedPhysical.has(e.code))'
HTML_HOTKEY = "if (e.code === 'Minus') { if (!e.repeat) { Atomics.store(keys, 0xE8, 1); trace('cash hotkey'); } return; }\n\t\t"


def leb(data, i):
    result = shift = 0
    while True:
        byte = data[i]; i += 1
        result |= (byte & 0x7f) << shift; shift += 7
        if byte < 0x80:
            return result, i


def sleb_at(data, i):
    result = shift = 0
    while True:
        byte = data[i]; i += 1
        result |= (byte & 0x7f) << shift; shift += 7
        if byte < 0x80:
            return result - (1 << shift) if byte & 0x40 else result, i


def enc_leb(value):
    out = bytearray()
    while True:
        byte = value & 0x7f; value >>= 7
        out.append(byte | (0x80 if value else 0))
        if not value:
            return bytes(out)


def sleb(value):
    out = bytearray()
    while True:
        byte = value & 0x7f; value >>= 7
        if (value == 0 and not byte & 0x40) or (value == -1 and byte & 0x40):
            return bytes(out + bytes([byte]))
        out.append(byte | 0x80)


def joaat(text):
    h = 0
    for c in text.lower().encode():
        h = (h + c) & 0xffffffff; h = (h + (h << 10)) & 0xffffffff; h ^= h >> 6
    h = (h + (h << 3)) & 0xffffffff; h ^= h >> 11
    return (h + (h << 15)) & 0xffffffff


def check(ok, what):
    if not ok:
        sys.exit(f'unexpected file: {what}')


def sections(data):
    check(data[:8] == b'\0asm\x01\0\0\0', 'wasm header')
    i, out = 8, {}
    while i < len(data):
        size, j = leb(data, i + 1)
        if data[i] == 0:
            n, k = leb(data, j)
            out[data[k:k + n].decode()] = (k + n, j + size)
        else:
            out[data[i]] = (i, j, j + size)
        i = j + size
    return out


def imported_functions(data, start, end):
    count, i = leb(data, start)
    funcs = 0
    for _ in range(count):
        for _ in range(2):  # module, field
            n, i = leb(data, i); i += n
        kind = data[i]; i += 1
        if kind == 0:
            _, i = leb(data, i); funcs += 1
        elif kind in (1, 2):  # table: reftype + limits; memory: limits
            i += kind == 1
            flags, i = leb(data, i); _, i = leb(data, i)
            if flags & 1:
                _, i = leb(data, i)
        elif kind == 3:
            i += 2
        else:  # tag: attribute + type index
            _, i = leb(data, i + 1)
    check(i == end, 'import section')
    return funcs


def function_names(data, start, end):
    i, names = start, {}
    while i < end:
        sub = data[i]; size, j = leb(data, i + 1)
        if sub == 1:
            count, k = leb(data, j)
            for _ in range(count):
                index, k = leb(data, k); n, k = leb(data, k)
                names.setdefault(data[k:k + n].decode(errors='replace'), []).append(index)
                k += n
        i = j + size
    return names


def patch_wasm(data):
    secs = sections(data)
    check('name' in secs and 2 in secs and 10 in secs, 'sections')
    first = imported_functions(data, secs[2][1], secs[2][2])
    names = function_names(data, *secs['name'])

    def index(name):
        found = names.get(name, [])
        check(len(found) == 1, name)
        return found[0]

    section_start, j, section_end = secs[10]
    count, p = leb(data, j)
    bodies, starts = [], []
    while p < section_end:
        size, q = leb(data, p)
        starts.append(q); bodies.append(bytearray(data[q:q + size]))
        p = q + size
    check(len(bodies) == count, 'code section')
    body = lambda name: bodies[index(name) - first]

    test_key = bytes(body(INPUT_TEST_KEY))
    at = test_key.find(bytes.fromhex('42ff0183 42'))  # i64.const 255; i64.and; i64.const BASE; i64.add
    check(at >= 0 and test_key.count(bytes.fromhex('42ff0183 42')) == 1, 'keyboard block')
    keyboard, k = sleb_at(test_key, at + 5)
    check(test_key[k] == 0x7c, 'keyboard block add')

    # __wasm_call_ctors builds the cash StatId_char from the five {SP0,SP1,SP2,MP0,MP1}_TOTAL_CASH hashes.
    hashes = [struct.unpack('<i', struct.pack('<I', joaat(f'{slot}_TOTAL_CASH')))[0] for slot in ('SP0', 'SP1', 'SP2', 'MP0', 'MP1')]
    offsets = (28, 24, 20, 16, 12)
    stores = b''.join(b'\x20\x00\x41' + sleb(h) + b'\x36\x02' + bytes([off]) for h, off in zip(hashes, offsets))
    ctor = [i for name, ids in names.items() if name.startswith(STAT_ID_CTOR) for i in ids]
    check(len(ctor) == 1, STAT_ID_CTOR)
    tail = b''.join(b'\x20\x00\x42' + sleb(off) + b'\x7c' for off in offsets) + b'\x10' + enc_leb(ctor[0])
    at = data.find(stores, j, section_end)
    check(at >= 0 and data.find(stores, at + 1, section_end) < 0 and data[at + len(stores)] == 0x42, 'cash stat')
    cash_stat, k = sleb_at(data, at + len(stores) + 1)
    check(data[k:k + len(tail)] == tail, 'cash stat constructor')

    i64 = lambda v: b'\x42' + sleb(v)
    call = lambda name: b'\x10' + enc_leb(index(name))
    flag = i64(keyboard + CASH_KEY)
    hotkey = b''.join((
        flag, bytes.fromhex('2d0000 0440'),                                   # if (flag)
        flag, bytes.fromhex('4100 3a0000'),                                   #   flag = 0
        bytes.fromhex('2300 4210 7d 2400'),                                   #   sp -= 16
        bytes.fromhex('2300 4208 7c'), i64(cash_stat), call(GET_STAT_ID),     #   id (at sp + 8) = the current character's cash stat
        bytes.fromhex('2300 4208 7c'), call(IS_KEY_VALID), bytes.fromhex('0440'),  # if (IsKeyValid(id))
        bytes.fromhex('2300 4208 7c 43'), struct.pack('<f', CASH_AMOUNT), bytes.fromhex('4103'), call(INCREMENT_STAT),
        bytes.fromhex('0b'),
        bytes.fromhex('2300 4210 7c 2400'),                                   #   sp += 16
        bytes.fromhex('0b'),
    ))
    update = body(STATS_UPDATE)
    check(update.count(UPDATE_PROLOGUE) == 1, 'CStatsMgr::Update prologue')
    count_locals, at = leb(update, 0)
    for _ in range(count_locals):
        _, at = leb(update, at); at += 1
    update[at:at] = hotkey

    payload = enc_leb(count) + b''.join(enc_leb(len(b)) + b for b in bodies)
    patched = data[:section_start] + bytes([10]) + enc_leb(len(payload)) + payload + data[section_end:]
    return patched, keyboard, cash_stat


def write(folder, rel, content):
    target = folder / 'overrides' / rel
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp = target.with_name(target.name + '.tmp')
    tmp.write_bytes(content)
    tmp.replace(target)
    print(f'{target}: {len(content)} bytes')


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    folder = Path(sys.argv[1])
    site = folder / 'mirror' / 'playgta5.com'
    html = (site / 'game.html').read_text(encoding='utf-8')
    check(html.count(HTML_ANCHOR) == 1, 'game.html keydown handler')
    at = html.index(HTML_ANCHOR) + len('e.preventDefault();\n\t\t')
    builds = sorted(site.glob('b/*/game.movie.wasm'))
    check(len(builds) == 1, 'one build folder')
    for variant in sys.argv[2:] or ['game.movie.wasm']:
        source = builds[0].parent / variant
        patched, keyboard, cash_stat = patch_wasm(source.read_bytes())
        print(f'{variant}: keyboard block {keyboard}, cash stat {cash_stat}')
        write(folder, source.relative_to(site), patched)
    write(folder, 'game.html', (html[:at] + HTML_HOTKEY + html[at:]).encode())


if __name__ == '__main__':
    main()
