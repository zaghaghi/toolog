// Read a screenshot with Vision and report where a pattern was found.
//
// Emits one JSON record per recognised line: the text, the line's box in image
// pixels with the origin at the top left, and — for every match of the pattern
// — the match's character range and the box Vision claims for it.
//
// **Do not trust the match box.** `boundingBox(for:)` is reliable enough at its
// left edge and wrong often enough at its right that a redaction built on it
// leaves the name legible; on this window's 13px monospace it sometimes hands
// back the whole line. It is emitted anyway because it is a useful check, but
// `redact.py` measures the span itself from the line's own pitch.
//
//   ocr <image.png> [regex]

import AppKit
import Foundation
import Vision

let path = CommandLine.arguments[1]
let pattern = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "Users[/\\-][A-Za-z0-9._]+"

guard let img = NSImage(contentsOfFile: path),
      let tiff = img.tiffRepresentation,
      let rep = NSBitmapImageRep(data: tiff),
      let cg = rep.cgImage else {
    FileHandle.standardError.write("cannot load \(path)\n".data(using: .utf8)!)
    exit(1)
}
let W = Double(cg.width), H = Double(cg.height)

let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = false   // these are shell commands, not prose
request.recognitionLanguages = ["en-US"]
if #available(macOS 13.0, *) { request.revision = VNRecognizeTextRequestRevision3 }
try VNImageRequestHandler(cgImage: cg, options: [:]).perform([request])

struct Match: Encodable { let text: String; let loc: Int; let len: Int; let mx: Double; let mw: Double }
struct Line: Encodable {
    let line: String
    let lx: Double, ly: Double, lw: Double, lh: Double
    let matches: [Match]
}

let re = try NSRegularExpression(pattern: pattern)
var out: [Line] = []

for obs in (request.results ?? []) {
    guard let cand = obs.topCandidates(1).first else { continue }
    let s = cand.string
    let ns = s as NSString
    let bb = obs.boundingBox
    var ms: [Match] = []
    for m in re.matches(in: s, range: NSRange(location: 0, length: ns.length)) {
        var mx = 0.0, mw = 0.0
        if let r = Range(m.range, in: s), let box = try? cand.boundingBox(for: r) {
            mx = box.boundingBox.minX * W
            mw = box.boundingBox.width * W
        }
        ms.append(Match(text: ns.substring(with: m.range),
                        loc: m.range.location, len: m.range.length, mx: mx, mw: mw))
    }
    out.append(Line(line: s, lx: bb.minX * W, ly: (1 - bb.maxY) * H,
                    lw: bb.width * W, lh: bb.height * H, matches: ms))
}
FileHandle.standardOutput.write(try JSONEncoder().encode(out))
