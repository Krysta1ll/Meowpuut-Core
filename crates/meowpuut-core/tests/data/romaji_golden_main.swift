// Writes "input<TAB>kana" for every line on stdin, using Romaji.reading from
// Sources/JapaneseBackend.swift. romaji_golden.tsv was made with it. From the repo root:
//   (echo 'import Foundation'; sed -n '/^enum Romaji {/,/^}/p' Sources/JapaneseBackend.swift) > /tmp/Romaji.swift
//   swiftc -O -parse-as-library -o /tmp/romaji-golden /tmp/Romaji.swift \
//       crates/meowpuut-core/tests/data/romaji_golden_main.swift
//   cut -f1 crates/meowpuut-core/tests/data/romaji_golden.tsv | /tmp/romaji-golden > new.tsv
import Foundation

@main
struct GoldenMain {
    static func main() {
        var output = ""
        while let line = readLine(strippingNewline: true) {
            output += line + "\t" + Romaji.reading(line) + "\n"
        }
        FileHandle.standardOutput.write(output.data(using: .utf8)!)
    }
}
