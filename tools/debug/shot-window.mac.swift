// Photograph one copy of the app's window: the one named, and no other.
//
// By the process, not by a region of the screen, for the same reason as
// shot-window.win.ps1: another copy, or another app, can sit on top of the
// copy being checked. screencapture -l takes the window's own picture,
// covered or not.
//
// Which copy is said by its process id, or by the folder its .app is in.
// The window is the largest one the process shows on screen: CEF keeps small
// windows of its own that are no use to look at.
//
// Prints the window's frame in points (x y width height, from the top left of
// the main screen), so a script can aim the mouse at a part of it.
//
// Taking a picture needs the Screen Recording permission for the app the
// shell runs in (Terminal, for instance). Without it screencapture says
// "could not create image from window".
//
//   swift tools/debug/shot-window.mac.swift --pid <id> --out shot.png
//   swift tools/debug/shot-window.mac.swift --under <folder> --out shot.png
//   swift tools/debug/shot-window.mac.swift --pid <id>        (frame only)
import AppKit
import CoreGraphics

func fail(_ s: String) -> Never {
    FileHandle.standardError.write((s + "\n").data(using: .utf8)!)
    exit(1)
}

var pid: pid_t?
var under: String?
var out: String?
var args = Array(CommandLine.arguments.dropFirst())
while !args.isEmpty {
    let a = args.removeFirst()
    guard let v = args.first else { fail("\(a) needs a value") }
    args.removeFirst()
    switch a {
    case "--pid": pid = pid_t(v)
    case "--under": under = (v as NSString).standardizingPath
    case "--out": out = v
    default: fail("unknown \(a)")
    }
}
if pid == nil, let under {
    pid = NSWorkspace.shared.runningApplications.first {
        guard let p = $0.executableURL?.path else { return false }
        return p.hasPrefix(under + "/") && p.hasSuffix("/MacOS/SHIKISHA-TERM")
    }?.processIdentifier
    if pid == nil { fail("no copy of the app is running under \(under)") }
}
guard let pid else { fail("say which copy: --pid <id> or --under <folder>") }

let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
var best: (id: CGWindowID, frame: CGRect)?
for w in list where (w[kCGWindowOwnerPID as String] as? pid_t) == pid
    && (w[kCGWindowLayer as String] as? Int) == 0 {
    guard let b = w[kCGWindowBounds as String] as! CFDictionary?,
          let r = CGRect(dictionaryRepresentation: b),
          let id = w[kCGWindowNumber as String] as? CGWindowID else { continue }
    if r.width * r.height > (best.map { $0.frame.width * $0.frame.height } ?? 0) { best = (id, r) }
}
guard let best else { fail("process \(pid) shows no window") }
print(Int(best.frame.minX), Int(best.frame.minY), Int(best.frame.width), Int(best.frame.height))

if let out {
    try? FileManager.default.createDirectory(atPath: (out as NSString).deletingLastPathComponent,
                                             withIntermediateDirectories: true)
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    // -o: without the window's shadow, so the picture is the window and no more
    p.arguments = ["-x", "-o", "-l\(best.id)", out]
    try p.run()
    p.waitUntilExit()
    if p.terminationStatus != 0 { fail("screencapture failed: is Screen Recording allowed for this terminal?") }
}
