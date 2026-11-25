#!/bin/bash
# Regenerates AppIcon.icns, preview-1024.png, preview-32.png and MenuBarIcon.pdf
# from AppIcon.svg and MenuBarIcon.svg. Needs only swift and iconutil (Xcode CLT).
set -euo pipefail
cd "$(dirname "$0")"

ICONSET="AppIcon.iconset"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP" "$ICONSET"' EXIT
rm -rf "$ICONSET"; mkdir "$ICONSET"

cat > "$TMP/render.swift" <<'SWIFT'
import AppKit

// usage: render png <in.svg> <out.png> <pixels>
//        render pdf <in.svg> <out.pdf> <points>
let a = CommandLine.arguments
guard a.count == 5, let size = Int(a[4]), let img = NSImage(contentsOfFile: a[2]) else {
    FileHandle.standardError.write("render: bad arguments or unreadable SVG\n".data(using: .utf8)!)
    exit(1)
}
let rect = NSRect(x: 0, y: 0, width: size, height: size)
if a[1] == "png" {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size,
                               bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    rep.size = rect.size
    NSGraphicsContext.saveGraphicsState()
    let ctx = NSGraphicsContext(bitmapImageRep: rep)!
    ctx.imageInterpolation = .high
    NSGraphicsContext.current = ctx
    img.draw(in: rect, from: .zero, operation: .copy, fraction: 1)
    NSGraphicsContext.restoreGraphicsState()
    try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[3]))
} else {
    var box = rect
    let pdf = CGContext(URL(fileURLWithPath: a[3]) as CFURL, mediaBox: &box, nil)!
    pdf.beginPDFPage(nil)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(cgContext: pdf, flipped: false)
    img.draw(in: rect, from: .zero, operation: .sourceOver, fraction: 1)
    NSGraphicsContext.restoreGraphicsState()
    pdf.endPDFPage()
    pdf.closePDF()
}
SWIFT
swiftc -O -o "$TMP/render" "$TMP/render.swift" 2>/dev/null || swiftc -o "$TMP/render" "$TMP/render.swift"

# Each size is rendered natively from the vector source (no downsampling).
for s in 16 32 128 256 512; do
    "$TMP/render" png AppIcon.svg "$ICONSET/icon_${s}x${s}.png" "$s"
    "$TMP/render" png AppIcon.svg "$ICONSET/icon_${s}x${s}@2x.png" "$((s * 2))"
done

cp "$ICONSET/icon_512x512@2x.png" preview-1024.png
cp "$ICONSET/icon_32x32.png" preview-32.png
iconutil -c icns "$ICONSET" -o AppIcon.icns

"$TMP/render" pdf MenuBarIcon.svg MenuBarIcon.pdf 18

echo "Built AppIcon.icns, preview-1024.png, preview-32.png, MenuBarIcon.pdf"
