#!/bin/bash
# Regenerates glyph-22.argb and glyph-44.argb (the tray icon, embedded into the
# binary) from ../../../macos/icon/MenuBarIcon.svg. Needs only swift (Xcode CLT, macOS).
#
# The files are raw ARGB32 pixels, one byte each in the order A R G B (network
# byte order, as the StatusNotifierItem spec wants), no header. The glyph is
# white on transparent because GNOME's top bar is dark; only the alpha channel
# of the black template SVG is used.
#
# devicehop.svg is a plain copy of ../../../macos/icon/AppIcon.svg.
set -euo pipefail
cd "$(dirname "$0")"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/render.swift" <<'SWIFT'
import AppKit

// usage: render <in.svg> <out.argb> <pixels>
let a = CommandLine.arguments
guard a.count == 4, let size = Int(a[3]), let img = NSImage(contentsOfFile: a[1]) else {
    FileHandle.standardError.write("render: bad arguments or unreadable SVG\n".data(using: .utf8)!)
    exit(1)
}
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size,
                           bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                           colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
let rect = NSRect(x: 0, y: 0, width: size, height: size)
rep.size = rect.size
NSGraphicsContext.saveGraphicsState()
let ctx = NSGraphicsContext(bitmapImageRep: rep)!
ctx.imageInterpolation = .high
NSGraphicsContext.current = ctx
img.draw(in: rect, from: .zero, operation: .copy, fraction: 1)
NSGraphicsContext.restoreGraphicsState()

var out = Data()
for y in 0..<size {
    for x in 0..<size {
        let alpha = UInt8((rep.colorAt(x: x, y: y)?.alphaComponent ?? 0) * 255 + 0.5)
        out.append(contentsOf: [alpha, 255, 255, 255])
    }
}
try! out.write(to: URL(fileURLWithPath: a[2]))
SWIFT
swiftc -O -o "$TMP/render" "$TMP/render.swift" 2>/dev/null || swiftc -o "$TMP/render" "$TMP/render.swift"

for s in 22 44; do
    "$TMP/render" ../../../macos/icon/MenuBarIcon.svg "glyph-$s.argb" "$s"
done
cp ../../../macos/icon/AppIcon.svg devicehop.svg

echo "Built glyph-22.argb, glyph-44.argb, devicehop.svg"
