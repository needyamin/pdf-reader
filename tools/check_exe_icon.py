"""Check whether a Windows PE file carries an icon resource.

Reads the resource directory directly: RT_ICON (type 3) holds the individual
images and RT_GROUP_ICON (type 14) is the group Windows actually picks from.
Presence of a group entry is what makes Explorer show a real icon.

Usage: python tools/check_exe_icon.py path/to/app.exe
"""

import struct
import sys


def rva_to_offset(sections, rva):
    for virtual, vsize, raw_ptr, raw_size in sections:
        if virtual <= rva < virtual + max(vsize, raw_size):
            return raw_ptr + (rva - virtual)
    return None


def resource_types(data, sections, res_rva):
    """Yield (type_id, name_id) for every leaf in the resource tree."""
    base = rva_to_offset(sections, res_rva)
    if base is None:
        return

    def walk(table_off, depth, path):
        # Header is 16 bytes: Characteristics(4) TimeDateStamp(4) Major(2)
        # Minor(2) NumberOfNamedEntries(2) NumberOfIdEntries(2).
        named, ids = struct.unpack_from("<HH", data, table_off + 12)
        for i in range(named + ids):
            entry = table_off + 16 + 8 * i
            name_field, offset_field = struct.unpack_from("<II", data, entry)
            sub = base + (offset_field & 0x7FFFFFFF)
            if offset_field & 0x80000000:
                yield from walk(sub, depth + 1, path + [name_field])
            else:
                leaf_rva, size, _ = struct.unpack_from("<III", data, sub)
                yield path + [name_field], leaf_rva, size

    yield from walk(base, 0, [])


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "target/debug/pdf-reader.exe"
    with open(path, "rb") as handle:
        data = handle.read()

    if data[:2] != b"MZ":
        print(f"not a PE file: {path}")
        return 1

    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe : pe + 4] != b"PE\0\0":
        print(f"no PE signature: {path}")
        return 1

    coff = pe + 4
    opt = coff + 20
    magic = struct.unpack_from("<H", data, opt)[0]
    is_pe32_plus = magic == 0x20B
    num_sections = struct.unpack_from("<H", data, coff + 2)[0]
    opt_size = struct.unpack_from("<H", data, coff + 16)[0]

    sections = []
    table = opt + opt_size
    for i in range(num_sections):
        off = table + 40 * i
        # Section header: Name(8) VirtualSize(4) VirtualAddress(4)
        # SizeOfRawData(4) PointerToRawData(4) ...
        _, vsize, virtual_addr, raw_size, raw_ptr = struct.unpack_from(
            "<8sIIII", data, off
        )
        sections.append((virtual_addr, vsize, raw_ptr, raw_size))

    # Data directory entry 2 is the resource table; the number of entries is
    # read from the optional header (92 for PE32, 108 for PE32+).
    # NumberOfRvaAndSizes sits just before the data directory, which itself
    # starts at opt+96 (PE32) or opt+112 (PE32+); entry 2 is the resource table.
    dir_start = opt + (112 if is_pe32_plus else 96)
    dir_count = struct.unpack_from("<I", data, dir_start - 4)[0]
    if dir_count < 3:
        print("no resource directory")
        return 1

    res_rva, res_size = struct.unpack_from("<II", data, dir_start + 16)

    icons, groups = [], []
    for path_ids, _rva, size in resource_types(data, sections, res_rva):
        if path_ids and path_ids[0] == 3:
            icons.append(size)
        if path_ids and path_ids[0] == 14:
            groups.append(size)

    print(f"file          : {path}")
    print(f"resource table: rva 0x{res_rva:x}, {res_size} bytes")
    print(f"RT_ICON       : {len(icons)} image(s), sizes {sorted(set(icons))}")
    print(f"RT_GROUP_ICON : {len(groups)} group(s)")
    if groups and icons:
        print("verdict       : icon resource present")
        return 0
    print("verdict       : NO icon resource")
    return 1


if __name__ == "__main__":
    sys.exit(main())
