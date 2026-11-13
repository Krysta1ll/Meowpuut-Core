import Foundation
// Build-time script conversion supplied by the platform, not an input-method engine.
while let line = readLine() {
    print(line.applyingTransform(StringTransform("Traditional-Simplified"), reverse: CommandLine.arguments.contains("--traditional")) ?? line)
}
