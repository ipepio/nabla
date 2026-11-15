"""Trim the transparent margin off a PNG (alpha bounding box).

No third-party libs here: the harness image has none installed and the assets
are 8-bit RGBA non-interlaced, so a small PNG reader/writer is enough.

Usage: python3 scripts/trim_png.py <input.png> <output.png>

Kept in the repo because brand assets are maintainer-supplied: re-running this
is how a new export gets its whitespace removed without opening a design tool.
"""

import struct
import sys
import zlib

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


class PngError(Exception):
    """Raised when the input is not a PNG this script can handle."""


def read_png(path):
    """Return (width, height, rgba bytes) for an 8-bit RGBA PNG."""
    try:
        with open(path, "rb") as handle:
            raw = handle.read()
    except OSError as error:
        raise PngError(f"cannot read {path}: {error}") from error

    if raw[:8] != PNG_MAGIC:
        raise PngError(f"{path} is not a PNG")

    pos, idat = 8, bytearray()
    width = height = None
    while pos < len(raw):
        (length,) = struct.unpack(">I", raw[pos : pos + 4])
        ctype = raw[pos + 4 : pos + 8]
        data = raw[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if ctype == b"IHDR":
            width, height, depth, color, _comp, _filt, interlace = struct.unpack(
                ">IIBBBBB", data
            )
            if depth != 8 or color != 6 or interlace != 0:
                raise PngError(f"{path} must be 8-bit RGBA, non-interlaced")
        elif ctype == b"IDAT":
            idat += data
        elif ctype == b"IEND":
            break

    if width is None or height is None:
        raise PngError(f"{path} has no IHDR chunk")
    return width, height, unfilter(zlib.decompress(bytes(idat)), width, height)


def unfilter(data, width, height):
    """Undo the per-scanline PNG filters (RGBA, 4 bytes per pixel)."""
    stride = width * 4
    out = bytearray()
    prev = bytearray(stride)
    pos = 0
    for _ in range(height):
        ftype = data[pos]
        line = bytearray(data[pos + 1 : pos + 1 + stride])
        pos += 1 + stride
        if ftype == 1:  # Sub
            for i in range(4, stride):
                line[i] = (line[i] + line[i - 4]) & 0xFF
        elif ftype == 2:  # Up
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif ftype == 3:  # Average
            for i in range(stride):
                left = line[i - 4] if i >= 4 else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif ftype == 4:  # Paeth
            for i in range(stride):
                a = line[i - 4] if i >= 4 else 0
                b = prev[i]
                c = prev[i - 4] if i >= 4 else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 0xFF
        out += line
        prev = line
    return out


def alpha_bbox(pixels, width, height, threshold=8):
    """Bounding box of pixels whose alpha is above `threshold`."""
    min_x, min_y, max_x, max_y = width, height, -1, -1
    for y in range(height):
        row = y * width * 4
        for x in range(width):
            if pixels[row + x * 4 + 3] > threshold:
                if x < min_x:
                    min_x = x
                if x > max_x:
                    max_x = x
                if y < min_y:
                    min_y = y
                if y > max_y:
                    max_y = y
    if max_x < 0:
        raise PngError("image is fully transparent")
    return min_x, min_y, max_x + 1, max_y + 1


def write_png(path, pixels, width, box):
    """Write the cropped region as a new 8-bit RGBA PNG."""
    x0, y0, x1, y1 = box
    w, h = x1 - x0, y1 - y0
    raw = bytearray()
    for y in range(y0, y1):
        raw.append(0)  # filter type 0 for every scanline
        start = (y * width + x0) * 4
        raw += pixels[start : start + w * 4]

    def chunk(ctype, data):
        body = ctype + data
        return (
            struct.pack(">I", len(data))
            + body
            + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    out = (
        PNG_MAGIC
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )
    try:
        with open(path, "wb") as handle:
            handle.write(out)
    except OSError as error:
        raise PngError(f"cannot write {path}: {error}") from error
    return w, h


def main(argv):
    if len(argv) != 3:
        raise SystemExit("usage: trim_png.py <input.png> <output.png>")
    try:
        width, height, pixels = read_png(argv[1])
        box = alpha_bbox(pixels, width, height)
        new_width, new_height = write_png(argv[2], pixels, width, box)
    except PngError as error:
        raise SystemExit(str(error)) from error
    print(f"{argv[1]} {width}x{height} -> {argv[2]} {new_width}x{new_height} {box}")


if __name__ == "__main__":
    main(sys.argv)
