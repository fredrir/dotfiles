// Ground truth for shared/zsh/conf.d/48-motion-keys.zsh, read straight out of
// AppKit so the shell does not have to guess what a Mac text view would do.
// Regenerate (macOS only; the checked-in .txt is what the suite reads):
//   swiftc -o /tmp/appkit-motion shared/zsh/tests/support/appkit-motion.swift
//   /tmp/appkit-motion > shared/zsh/tests/support/appkit-motion.txt
import AppKit
let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
let tv = NSTextView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))

let cases = [
  "123 45 67 ", "123 45 67", "123\n45\n   ", "123\n45\n", "123\n45",
  "/opt/dotfiles/shared/wezterm", "foo_bar-baz", "hello, world!",
  "a  b", "foo.bar.baz", "  leading", "trailing;;;   ",
  "æøå Norsk-tekst", "one two\nthree", "http://x.com/y", "don't stop", "1.5 kg",
  "123 456 ", "123 456  ", "123 456   ", "123\t\t456", "a b  c",
]

func esc(_ s: String) -> String {
  s.replacingOccurrences(of: "\\", with: "\\\\")
   .replacingOccurrences(of: "\n", with: "\\n")
   .replacingOccurrences(of: "'", with: "'\\''")
}

for sel in ["deleteWordBackward:", "deleteToBeginningOfLine:"] {
  print("# \(sel)")
  for c in cases {
    tv.string = c
    tv.setSelectedRange(NSRange(location: (c as NSString).length, length: 0))
    var steps: [String] = []
    for _ in 0..<5 { tv.perform(Selector(sel)); steps.append(tv.string) }
    print("'\(esc(c))' '\(steps.map { esc($0) }.joined(separator: "|"))'")
  }
  print("")
}
