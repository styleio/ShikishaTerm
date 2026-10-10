// Move, click, double-click or drag the mouse, for checks a person would
// otherwise do by hand: the window's top bar dragged, double-clicked.
//
// Points are in screen points from the top left of the main screen, the same
// as shot-window.mac.swift prints.
//
// Posting mouse events needs the Accessibility permission for the app the
// shell runs in (Terminal, for instance). Without it the events are dropped
// without a word, so this checks first and says so.
//
//   swift tools/debug/mouse.mac.swift click <x> <y>
//   swift tools/debug/mouse.mac.swift double <x> <y>
//   swift tools/debug/mouse.mac.swift drag <x1> <y1> <x2> <y2>
import ApplicationServices
import Foundation

func fail(_ s: String) -> Never {
    FileHandle.standardError.write((s + "\n").data(using: .utf8)!)
    exit(1)
}
if !AXIsProcessTrusted() { fail("Accessibility is not allowed for this terminal") }

let a = Array(CommandLine.arguments.dropFirst())
guard let verb = a.first else { fail("say click, double or drag") }
let n = a.dropFirst().compactMap { Double($0) }

func post(_ type: CGEventType, _ p: CGPoint, clicks: Int64 = 1) {
    let e = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: p, mouseButton: .left)!
    e.setIntegerValueField(.mouseEventClickState, value: clicks)
    e.post(tap: .cghidEventTap)
    usleep(30_000)
}

switch (verb, n.count) {
case ("click", 2):
    let p = CGPoint(x: n[0], y: n[1])
    post(.mouseMoved, p); post(.leftMouseDown, p); post(.leftMouseUp, p)
case ("double", 2):
    let p = CGPoint(x: n[0], y: n[1])
    post(.mouseMoved, p)
    post(.leftMouseDown, p, clicks: 1); post(.leftMouseUp, p, clicks: 1)
    post(.leftMouseDown, p, clicks: 2); post(.leftMouseUp, p, clicks: 2)
case ("drag", 4):
    let from = CGPoint(x: n[0], y: n[1]), to = CGPoint(x: n[2], y: n[3])
    post(.mouseMoved, from); post(.leftMouseDown, from)
    // In steps, as a hand would: a single jump is not always taken as a drag
    for i in 1...20 {
        let t = Double(i) / 20
        post(.leftMouseDragged, CGPoint(x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t))
    }
    post(.leftMouseUp, to)
default:
    fail("click <x> <y> | double <x> <y> | drag <x1> <y1> <x2> <y2>")
}
