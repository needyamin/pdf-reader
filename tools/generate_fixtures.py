#!/usr/bin/env python3
"""Generate deterministic PDF fixtures for tests and benchmarks.

Deliberately dependency-free: these are plain uncompressed PDFs using the
standard Helvetica base font, so the fixtures are reproducible byte-for-byte
and can be regenerated on any machine without a PDF library.

An encrypted fixture is also produced using the PDF standard security handler
(revision 2, RC4 with a 5-byte key) so the engine's password path can be
exercised end to end. The encryption here is minimal on purpose: only the page
content streams are encrypted, which is all PDFium needs to require a password
to open the file.
"""

from __future__ import annotations

import argparse
import hashlib
import struct
import sys
from pathlib import Path

PAGE_WIDTH = 595.28
PAGE_HEIGHT = 841.89

WORDS = (
    "the quick brown fox jumps over the lazy dog while rendering tiles "
    "across a very large document to measure rasterization throughput and "
    "cache behaviour under continuous scrolling at high device pixel ratios"
).split()

# PDF 1.7 standard security-handler padding string (32 bytes).
PADDING = bytes(
    [
        0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41,
        0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
        0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80,
        0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
    ]
)

# Fully permissive permission word (all permission bits set).
PERMISSIONS = -4

# Password baked into the encrypted fixture.
ENCRYPTED_PASSWORD = "secret"


def pad_password(password: str) -> bytes:
    """Pad or truncate a password to exactly 32 bytes with the padding string."""
    data = password.encode("latin-1")[:32]
    return data + PADDING[len(data):]


def rc4(key: bytes, data: bytes) -> bytes:
    """RC4 stream cipher (key may be shorter than the state; that is intended)."""
    state = list(range(256))
    index = 0
    for i in range(256):
        index = (index + state[i] + key[i % len(key)]) & 0xFF
        state[i], state[index] = state[index], state[i]

    out = bytearray()
    i = j = 0
    for byte in data:
        i = (i + 1) & 0xFF
        j = (j + state[i]) & 0xFF
        state[i], state[j] = state[j], state[i]
        out.append(byte ^ state[(state[i] + state[j]) & 0xFF])
    return bytes(out)


def _extend_key(seed: bytes) -> bytes:
    """Algorithm 3.2 (revision >= 3): iterate MD5 50 times to stretch a key."""
    key = seed
    for _ in range(50):
        key = hashlib.md5(key).digest()
    return key


def compute_o(user_password: str, owner_password: str, permissions: int) -> bytes:
    """Standard security handler, Algorithm 3.3: derive the O value (rev 3)."""
    owner_padded = pad_password(owner_password)
    user_padded = pad_password(user_password)
    seed = hashlib.md5(
        owner_padded + user_padded + struct.pack("<i", permissions)
    ).digest()
    return rc4(_extend_key(seed), PADDING)


def compute_u(user_password: str, permissions: int, o: bytes) -> bytes:
    """Standard security handler, Algorithm 3.4: derive the U value (rev 3)."""
    user_padded = pad_password(user_password)
    seed = hashlib.md5(
        user_padded + o + struct.pack("<i", permissions)
    ).digest()
    key = _extend_key(seed)
    value = PADDING
    for _ in range(20):
        value = rc4(key, value)
    return value


def document_key(user_password: str, o: bytes, permissions: int) -> bytes:
    """Standard security handler, Algorithm 3.2: the document encryption key (rev 3)."""
    user_padded = pad_password(user_password)
    seed = hashlib.md5(
        user_padded + o + struct.pack("<i", permissions)
    ).digest()
    return _extend_key(seed)


def page_content(page_index: int, lines_per_page: int) -> bytes:
    """Build the content stream for one page."""
    out = ["BT", "/F1 11 Tf", "14 TL", f"1 0 0 1 56 {PAGE_HEIGHT - 64} Tm"]

    for line in range(lines_per_page):
        start = (page_index * lines_per_page + line) * 9
        words = [WORDS[(start + i) % len(WORDS)] for i in range(9)]
        text = " ".join(words).replace("\\", r"\\").replace("(", r"\(").replace(")", r"\)")
        out.append(f"({text}) Tj")
        out.append("T*")

    out.append("ET")
    return "\n".join(out).encode("latin-1")


def build_pdf(page_count: int, lines_per_page: int, encrypt: dict | None = None) -> bytes:
    """Assemble a complete PDF document with `page_count` pages.

    When `encrypt` is provided it must contain `o`, `u`, `p` and `key`; the
    page content streams are then RC4-encrypted and an /Encrypt dictionary is
    added and referenced from the trailer.
    """
    objects: list[bytes] = []

    font_obj = 3
    first_page_obj = 4

    page_obj_ids = [first_page_obj + (2 * i) for i in range(page_count)]
    content_obj_ids = [first_page_obj + (2 * i) + 1 for i in range(page_count)]

    kids = " ".join(f"{oid} 0 R" for oid in page_obj_ids)

    objects.append(b"<< /Type /Catalog /Pages 2 0 R >>")
    objects.append(
        f"<< /Type /Pages /Kids [{kids}] /Count {page_count} >>".encode("latin-1")
    )
    objects.append(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
    )

    streams: dict[int, bytes] = {}

    for i in range(page_count):
        objects.append(
            (
                f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_WIDTH} {PAGE_HEIGHT}] "
                f"/Resources << /Font << /F1 {font_obj} 0 R >> >> "
                f"/Contents {content_obj_ids[i]} 0 R >>"
            ).encode("latin-1")
        )

        data = page_content(i, lines_per_page)
        if encrypt is not None:
            data = rc4(encrypt["key"], data)
        objects.append(b"")  # placeholder, filled after we know the offset
        streams[len(objects)] = data

    encrypt_num = None
    if encrypt is not None:
        encrypt_num = len(objects) + 1
        encrypt_obj = (
            f"<< /Filter /Standard /V 2 /R 3 /Length 128 "
            f"/O <{encrypt['o'].hex()}> /U <{encrypt['u'].hex()}> /P {encrypt['p']} >>"
        )
        objects.append(encrypt_obj.encode("latin-1"))

    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")

    offsets: list[int] = []

    for number, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += f"{number} 0 obj\n".encode("latin-1")

        if number in streams:
            data = streams[number]
            out += f"<< /Length {len(data)} >>\nstream\n".encode("latin-1")
            out += data
            out += b"\nendstream"
        else:
            out += body

        out += b"\nendobj\n"

    xref_offset = len(out)
    total = len(objects) + 1

    out += f"xref\n0 {total}\n".encode("latin-1")
    out += b"0000000000 65535 f \n"
    for offset in offsets:
        out += f"{offset:010d} 00000 n \n".encode("latin-1")

    encrypt_ref = f" /Encrypt {encrypt_num} 0 R" if encrypt_num is not None else ""
    out += (
        f"trailer\n<< /Size {total} /Root 1 0 R{encrypt_ref} >>\n"
        f"startxref\n{xref_offset}\n%%EOF\n"
    ).encode("latin-1")

    return bytes(out)


def build_form_pdf() -> bytes:
    """Assemble a one-page PDF with an AcroForm covering every widget type.

    Every widget carries a real /AP appearance stream: without one PDFium has
    nothing to rasterize, so the Forms UI would render empty boxes and the
    fixture would not test what users actually see.
    """
    W = 595.28
    H = 841.89
    objects: list[bytes] = []

    # Object numbering is fixed up front so cross references stay readable.
    # 1 catalog, 2 pages, 3 acroform, 4 page, 5 font, 6 contents,
    # 7 text widget, 8 text AP, 9 checkbox widget, 10 checkbox "yes" AP,
    # 11 checkbox "off" AP, 12 radio parent, 13 radio AP, 14/15 radio kids,
    # 16 combo widget, 17 combo AP, 18 list widget, 19 list AP,
    # 20 pushbutton widget, 21 pushbutton AP.
    FIELDS = [7, 9, 12, 16, 18, 20]
    ANNOTS = [7, 9, 14, 15, 16, 18, 20]

    objects.append(
        f"<< /Type /Catalog /Pages 2 0 R /AcroForm 3 0 R >>".encode("latin-1")
    )
    objects.append(b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>")
    objects.append(
        (
            f"<< /Fields [{' '.join(f'{n} 0 R' for n in FIELDS)}] "
            f"/DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> "
            f"/NeedAppearances true >>"
        ).encode("latin-1")
    )
    objects.append(
        (
            f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] "
            f"/Resources << /Font << /Helv 5 0 R >> >> /Contents 6 0 R "
            f"/Annots [{' '.join(f'{n} 0 R' for n in ANNOTS)}] >>"
        ).encode("latin-1")
    )
    objects.append(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
    )

    labels = [
        ("Full name", 748.0),
        ("Subscribe", 706.0),
        ("Colour", 664.0),
        ("Delivery", 618.0),
        ("Toppings", 566.0),
    ]
    lines = ["BT", "/F1 10 Tf", "1 0 0 1 60 790 Tm", "(Sample form) Tj", "ET"]
    for text, y in labels:
        lines.append("BT")
        lines.append(f"1 0 0 1 60 {y + 24:.2f} Tm")
        lines.append(f"({text}) Tj")
        lines.append("ET")
    contents = "\n".join(lines).encode("latin-1")

    # Placeholder object so /Contents is 6 0 R; the real stream is patched in
    # below before serialisation.
    objects.append(b"")
    contents_slot = len(objects)

    def ap(number: int, width: float, height: float, stream: str) -> None:
        body = stream.encode("latin-1")
        header = (
            f"<< /Type /XObject /Subtype /Form /BBox [0 0 {width} {height}] "
            f"/Length {len(body)} >>\nstream\n"
        ).encode("latin-1")
        assert number == len(objects) + 1, f"object {number} out of order"
        objects.append(b"")
        streams[len(objects)] = header + body + b"endstream"

    streams: dict[int, bytes] = {}

    def widget(extra: str) -> str:
        return (
            f"<< /Type /Annot /Subtype /Widget /F 4 /P 4 0 R {extra} >>"
        )

    # 7/8 — text field, pre-filled so the value path is exercised.
    objects.append(
        widget(
            "/FT /Tx /T (FullName) /TU (Enter your full name) /V (Jane Doe) "
            "/DA (/Helv 10 Tf 0 g) /Rect [60 728 300 748] "
            "/AP << /N 8 0 R >>"
        ).encode("latin-1")
    )
    ap(8, 240, 20, "0.6 0.6 0.6 RG 0.75 w 0.375 0.375 239.25 19.25 re S "
                   "BT /F1 10 Tf 1 0 0 1 3 5 Tm (Jane Doe) Tj ET")

    # 9/10/11 — checkbox, checked.
    objects.append(
        widget(
            "/FT /Btn /T (Subscribe) /TU (Subscribe to the newsletter) /V /Yes "
            "/AS /Yes /Rect [60 688 78 706] /AP << /N << /Yes 10 0 R /Off 11 0 R >> >>"
        ).encode("latin-1")
    )
    ap(10, 18, 18, "0 0 0 RG 0.75 w 0.375 0.375 17.25 17.25 re S "
                   "2.5 2.5 m 15.5 15.5 l 15.5 2.5 m 2.5 15.5 l S")
    ap(11, 18, 18, "0 0 0 RG 0.75 w 0.375 0.375 17.25 17.25 re S")

    # 12/13/14/15 — a radio group: one parent field, two kid widgets.
    objects.append(
        b"<< /FT /Btn /Ff 32768 /T (Colour) /V /Red /Kids [14 0 R 15 0 R] >>"
    )
    ap(13, 18, 18, "0 0 0 RG 0.75 w 0.375 0.375 17.25 17.25 re "
                   "9 9 7.5 0 360 arc f")
    objects.append(
        widget(
            "/Parent 12 0 R /AS /Red /Rect [60 646 78 664] "
            "/AP << /N << /Red 13 0 R /Off 13 0 R >> >>"
        ).encode("latin-1")
    )
    objects.append(
        widget(
            "/Parent 12 0 R /AS /Off /Rect [60 620 78 638] "
            "/AP << /N << /Green 13 0 R /Off 13 0 R >> >>"
        ).encode("latin-1")
    )

    # 16/17 — combo box with a current selection.
    objects.append(
        widget(
            "/FT /Ch /T (Delivery) /TU (Choose a delivery method) /Ff 131072 "
            "/Opt [(Email) (Courier) (Pickup)] /V (Courier) "
            "/Rect [60 598 240 618] /AP << /N 17 0 R >>"
        ).encode("latin-1")
    )
    ap(17, 180, 20, "0.6 0.6 0.6 RG 0.75 w 0.375 0.375 179.25 19.25 re S "
                    "BT /F1 10 Tf 1 0 0 1 3 5 Tm (Courier) Tj ET")

    # 19/20 — list box.
    objects.append(
        widget(
            "/FT /Ch /T (Toppings) /Ff 0 /Opt [(Cheese) (Tomato) (Basil)] "
            "/V (Cheese) /Rect [60 546 240 606] /AP << /N 19 0 R >>"
        ).encode("latin-1")
    )
    ap(19, 180, 60, "0.6 0.6 0.6 RG 0.75 w 0.375 0.375 179.25 59.25 re S "
                    "BT /F1 10 Tf 1 0 0 1 4 44 Tm (Cheese) Tj ET "
                    "1 0 0 1 4 24 Tm (Tomato) Tj ET "
                    "1 0 0 1 4 4 Tm (Basil) Tj ET")

    # 21/22 — push button, no persistent value.
    objects.append(
        widget(
            "/FT /Btn /Ff 65536 /T (Submit) /TU (Submit the form) "
            "/Rect [60 500 150 522] /AP << /N 21 0 R >>"
        ).encode("latin-1")
    )
    ap(21, 90, 22, "0.85 0.85 0.9 rg 0.375 0.375 89.25 21.25 re f "
                   "BT /F1 10 Tf 1 0 0 1 24 7 Tm (Submit) Tj ET")

    streams[contents_slot] = (
        f"<< /Length {len(contents)} >>\nstream\n".encode("latin-1")
        + contents
        + b"\nendstream"
    )

    return serialise(objects, streams, trailer_extra="")


def serialise(objects: list[bytes], streams: dict[int, bytes], trailer_extra: str = "") -> bytes:
    """Write objects, an xref table and a trailer.

    A `streams` entry replaces the object body entirely: it is already a
    complete `<< /Length ... >> stream ... endstream` block, because appearance
    streams need their own dictionary (/Type /XObject /BBox) and cannot share
    the plain content-stream wrapper.
    """
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets: list[int] = []
    for number, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += f"{number} 0 obj\n".encode("latin-1")
        out += streams.get(number, body)
        out += b"\nendobj\n"

    xref_offset = len(out)
    total = len(objects) + 1
    out += f"xref\n0 {total}\n".encode("latin-1")
    out += b"0000000000 65535 f \n"
    for offset in offsets:
        out += f"{offset:010d} 00000 n \n".encode("latin-1")
    out += (
        f"trailer\n<< /Size {total} /Root 1 0 R{trailer_extra} >>\n"
        f"startxref\n{xref_offset}\n%%EOF\n"
    ).encode("latin-1")
    return bytes(out)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("outdir", type=Path, help="directory to write fixtures into")
    parser.add_argument("--large-pages", type=int, default=500, help="page count for the large fixture")
    parser.add_argument(
        "--encrypted-password",
        default=ENCRYPTED_PASSWORD,
        help="user password baked into the encrypted fixture",
    )
    args = parser.parse_args()

    args.outdir.mkdir(parents=True, exist_ok=True)

    small = args.outdir / "small-3p.pdf"
    large = args.outdir / "large-500p.pdf"
    encrypted = args.outdir / "encrypted.pdf"
    forms = args.outdir / "forms.pdf"

    small.write_bytes(build_pdf(3, 40))
    large.write_bytes(build_pdf(args.large_pages, 40))
    forms.write_bytes(build_form_pdf())

    o = compute_o(args.encrypted_password, "", PERMISSIONS)
    u = compute_u(args.encrypted_password, PERMISSIONS, o)
    key = document_key(args.encrypted_password, o, PERMISSIONS)
    encrypted.write_bytes(
        build_pdf(3, 40, encrypt={"o": o, "u": u, "p": PERMISSIONS, "key": key})
    )

    print(f"wrote {small} ({small.stat().st_size} bytes)")
    print(f"wrote {large} ({large.stat().st_size} bytes)")
    print(f"wrote {forms} ({forms.stat().st_size} bytes)")
    print(f"wrote {encrypted} ({encrypted.stat().st_size} bytes, password={args.encrypted_password!r})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
