// Draws assets/app-icon.png from assets/logo.png.
//
// The phoenix on a dark tile drawn to Apple's icon grid: an 824 point rounded square with
// continuous corners, centred in 1024. macOS 26 puts any icon that does not fill that shape
// on a grey plate of its own and shrinks it to fit, which is what the bare logo got. The
// tile here is that plate's own gradient, so the icon keeps the look and the bird gets the
// room.
//
// Run by hand when the logo changes, with `just app-icon`, and never by a build: it needs
// Xcode's `swift`, which neither the Homebrew formula nor a Nix build may assume. The
// result is committed, and both bundle builders resize it.

import AppKit
import SwiftUI
import UniformTypeIdentifiers

let args = CommandLine.arguments
let logoPath = args[1]
let outPath = args[2]

/// How much of the tile the bird spans. The plate macOS drew gave it 45 percent.
let share: CGFloat = 0.66

let canvas = 1024
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let sRGB = CGColorSpace(name: CGColorSpace.sRGB)!

func color(_ hex: UInt32) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
        green: CGFloat((hex >> 8) & 0xff) / 255,
        blue: CGFloat(hex & 0xff) / 255,
        alpha: 1)
}

func context(_ width: Int, _ height: Int) -> CGContext {
    CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: sRGB, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
}

let logo = NSImage(contentsOfFile: logoPath)!.cgImage(forProposedRect: nil, context: nil, hints: nil)!

// Crop to the bird, since the logo carries wide margins of its own.
let probe = context(logo.width, logo.height)
probe.draw(logo, in: CGRect(x: 0, y: 0, width: logo.width, height: logo.height))
let pixels = probe.data!.assumingMemoryBound(to: UInt8.self)
var (minX, minY, maxX, maxY) = (Int.max, Int.max, 0, 0)
for y in 0..<logo.height {
    for x in 0..<logo.width where pixels[y * probe.bytesPerRow + x * 4 + 3] > 8 {
        minX = min(minX, x)
        maxX = max(maxX, x)
        minY = min(minY, y)
        maxY = max(maxY, y)
    }
}
let bird = logo.cropping(
    to: CGRect(x: minX, y: minY, width: maxX - minX + 1, height: maxY - minY + 1))!

let scale = tile.width * share / CGFloat(max(bird.width, bird.height))
let size = CGSize(width: CGFloat(bird.width) * scale, height: CGFloat(bird.height) * scale)
// A touch above centre, where the eye puts the middle of a shape.
let birdRect = CGRect(
    x: tile.midX - size.width / 2,
    y: tile.midY - size.height / 2 + tile.height * 0.015,
    width: size.width, height: size.height)

let out = context(canvas, canvas)
out.interpolationQuality = .high
out.addPath(RoundedRectangle(cornerRadius: 185.4, style: .continuous).path(in: tile).cgPath)
out.clip()
let plate = CGGradient(
    colorsSpace: sRGB, colors: [color(0x313131), color(0x141414)] as CFArray, locations: [0, 1])!
out.drawLinearGradient(
    plate, start: CGPoint(x: 0, y: tile.maxY), end: CGPoint(x: 0, y: tile.minY), options: [])
out.draw(bird, in: birdRect)

let destination = CGImageDestinationCreateWithURL(
    URL(fileURLWithPath: outPath) as CFURL, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(destination, out.makeImage()!, nil)
CGImageDestinationFinalize(destination)
print("wrote \(outPath), bird \(Int(size.width))x\(Int(size.height)) in an 824 tile")
