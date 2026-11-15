// Runs Tests/skin-vectors/bundled/index.json loadCases through SkinSealVerifier.swift (see its header).
import Foundation

let vectors = URL(fileURLWithPath: CommandLine.arguments[1])
let index = try! JSONSerialization.jsonObject(with: Data(contentsOf: vectors.appendingPathComponent("index.json"))) as! [String: Any]
let keys = index["keys"] as! [String: Any]
let root = keys["root"] as! [String: Any]
let rootKey = [UInt8](Data(base64Encoded: root["public"] as! String)!)
let fm = FileManager.default
var failures = 0
var count = 0
for case_ in index["loadCases"] as! [[String: Any]] {
    let name = case_["name"] as! String
    let tmp = fm.temporaryDirectory.appendingPathComponent("seal-\(UUID().uuidString)")
    try! fm.copyItem(at: vectors.appendingPathComponent("installed"), to: tmp)
    for op in case_["ops"] as! [[String: Any]] {
        let path = tmp.appendingPathComponent(op["path"] as! String)
        switch op["op"] as! String {
        case "flip":
            var d = [UInt8](try! Data(contentsOf: path)); d[(op["offset"] as! NSNumber).intValue] ^= 1
            try! Data(d).write(to: path)
        case "truncate":
            let d = try! Data(contentsOf: path); try! d.prefix((op["length"] as! NSNumber).intValue).write(to: path)
        case "write":
            var d = (op["append"] as? Bool ?? false) ? try! Data(contentsOf: path) : Data()
            d.append(contentsOf: SkinSeal.hex(op["hex"] as! String)); try! d.write(to: path)
        case "delete": try! fm.removeItem(at: path)
        case "move": try! fm.moveItem(at: path, to: tmp.appendingPathComponent(op["to"] as! String))
        case "symlink": try! fm.createSymbolicLink(atPath: path.path, withDestinationPath: op["target"] as! String)
        case "copy":
            try! fm.removeItem(at: path)
            try! fm.copyItem(at: vectors.appendingPathComponent(op["from"] as! String), to: path)
        default: fatalError("op")
        }
    }
    let test = (case_["policy"] as! String) == "test"
    let verifier = SkinSeal(roots: test ? [root["kid"] as! String: rootKey] : [:], acceptTestKeys: test)
    var stateData: Data? = nil
    if let s = case_["state"] { stateData = try! JSONSerialization.data(withJSONObject: s) }
    let state = SkinState(json: stateData)!
    let result = verifier.verifyInstalled(skinsDir: tmp, customSkin: case_["skin"] as? String ?? "sakura_tea@3", state: state)
    let got: Int32
    switch result { case .success: got = 0; case .failure(let c): got = c.rawValue }
    let expect = (case_["expect"] as! NSNumber).int32Value
    print(got == expect ? "ok  " : "FAIL", name, "expect", expect, "got", got)
    if got != expect { failures += 1 }
    count += 1
    try? fm.removeItem(at: tmp)
}
print("\(count) cases, \(failures) failures")
exit(failures == 0 ? 0 : 1)
