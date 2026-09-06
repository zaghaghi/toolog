// Move and scroll the pointer, because System Events cannot.
//
// `click at {x, y}` and any other pointer control in System Events fails with
// -25208 under this harness, and the window's contents are a WebView, so the
// two things a screenshot needs — no `:hover` band baked into a row, and a list
// scrolled to a chosen offset — have to come from CoreGraphics events.
//
// Warping alone is not enough. `CGWarpMouseCursorPosition` moves the cursor
// without telling WebKit, which keeps painting `:hover` on whatever the last
// event touched; the pointer can sit on the Dock while a row three hundred
// pixels inside the window stays lit. Every warp here is followed by a real
// `mouseMoved` so the hover state actually moves with it.
//
//   mouse move    <x> <y>
//   mouse scroll  <x> <y> <lines>  <steps>
//   mouse scrollpx <x> <y> <pixels> <steps>
//
// Scroll amounts are positive up, negative down. Pixel units are what you want
// for a list of fixed-height rows: 28 px a row means an exact number of rows,
// and no row is left clipped in half at the top of the frame.

import CoreGraphics
import Foundation

let a = CommandLine.arguments
guard a.count >= 4 else {
    FileHandle.standardError.write("usage: mouse move|scroll|scrollpx x y [amount steps]\n".data(using: .utf8)!)
    exit(2)
}

func post(_ e: CGEvent?) { e?.post(tap: .cghidEventTap) }

func warp(_ x: Double, _ y: Double) {
    let p = CGPoint(x: x, y: y)
    CGWarpMouseCursorPosition(p)
    post(CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: .left))
}

switch a[1] {
case "move":
    warp(Double(a[2])!, Double(a[3])!)
case "scroll", "scrollpx":
    warp(Double(a[2])!, Double(a[3])!)
    usleep(200_000)
    let units: CGScrollEventUnit = a[1] == "scroll" ? .line : .pixel
    let amount = Int32(a[4])!
    for _ in 0..<Int(a[5])! {
        post(CGEvent(scrollWheelEvent2Source: nil, units: units, wheelCount: 1,
                     wheel1: amount, wheel2: 0, wheel3: 0))
        usleep(50_000)
    }
default:
    FileHandle.standardError.write("unknown verb \(a[1])\n".data(using: .utf8)!)
    exit(2)
}
