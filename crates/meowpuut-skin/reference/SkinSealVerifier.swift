// Reference implementation of ../SEAL.md (bundled seals only) with CryptoKit and
// Foundation, no Rust. A template for the iOS keyboard, not built by cargo. Check it
// against the shared vectors (macOS):
//   swiftc -O SkinSealVerifier.swift main.swift -o /tmp/skin-seal && /tmp/skin-seal ../../../Tests/skin-vectors/bundled
import CryptoKit
import Foundation

public enum SkinCode: Int32, Error {
    case tooLarge = 1, malformed = 2, header = 3, certificate = 4, keyUse = 5, testKey = 6, revoked = 7,
         signature = 8, downgrade = 18, notInstalled = 23, tampered = 24, argument = 26, kind = 32
}

public struct SkinState {
    public var highest: [String: UInt32] = [:]
    public var revokedKids: Set<String> = []
    public var revokedSkins: Set<String> = []
    public var revokedSerials: Set<String> = []

    public init() {}

    /// The platform's state JSON ("" or nil: none).
    public init?(json: Data?) {
        guard let json, !json.isEmpty else { return }
        guard let o = try? JSONSerialization.jsonObject(with: json) as? [String: Any],
              Set(o.keys) == ["highest", "revoked", "seq", "v"],
              (o["v"] as? NSNumber)?.intValue == 1,
              let h = o["highest"] as? [String: Any],
              let r = o["revoked"] as? [String: Any], Set(r.keys) == ["kid", "serial", "skin"] else { return nil }
        for (k, v) in h {
            guard SkinSeal.isName(k), let n = SkinSeal.u32(v), n >= 1 else { return nil }
            highest[k] = n
        }
        guard let kids = r["kid"] as? [String], let serials = r["serial"] as? [String], let skins = r["skin"] as? [String] else { return nil }
        revokedKids = Set(kids); revokedSerials = Set(serials); revokedSkins = Set(skins)
    }
}

public struct InstalledSkin {
    public let id: String
    public let rev: UInt32
    public let dir: URL
    public let manifest: [String: Any]
}

public struct SkinSeal {
    public static let bundledMagic: [UInt8] = Array("MPSKBN".utf8) + [0, 1]
    public static let recipientMagic: [UInt8] = Array("MPSKIN".utf8) + [0, 2]
    static let bundledDomain = Array("meowpuut-skin-bundled/v1".utf8) + [0]
    static let certDomain = Array("meowpuut-cert/v2".utf8) + [0]
    static let maxSeal = 9358
    static let ell: [UInt8] = hex("edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010")
    static let smallOrder: Set<[UInt8]> = Set([
        "0000000000000000000000000000000000000000000000000000000000000000",
        "0000000000000000000000000000000000000000000000000000000000000080",
        "0100000000000000000000000000000000000000000000000000000000000000",
        "0100000000000000000000000000000000000000000000000000000000000080",
        "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05",
        "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85",
        "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a",
        "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa",
    ].map(hex))

    public let roots: [String: [UInt8]]
    public let acceptTestKeys: Bool

    public init(roots: [String: [UInt8]], acceptTestKeys: Bool) {
        self.roots = roots
        self.acceptTestKeys = acceptTestKeys
    }

    static func hex(_ s: String) -> [UInt8] {
        var out = [UInt8](); var i = s.startIndex
        while i < s.endIndex { let j = s.index(i, offsetBy: 2); out.append(UInt8(s[i..<j], radix: 16)!); i = j }
        return out
    }

    static func hexString(_ b: [UInt8]) -> String { b.map { String(format: "%02x", $0) }.joined() }

    static func isName(_ s: String) -> Bool {
        let b = Array(s.utf8)
        guard (1...64).contains(b.count), b.allSatisfy({ ($0 >= 97 && $0 <= 122) || ($0 >= 48 && $0 <= 57) || $0 == 95 || $0 == 45 }) else { return false }
        if ["con", "prn", "aux", "nul"].contains(s) { return false }
        if b.count == 4, s.hasPrefix("com") || s.hasPrefix("lpt"), (49...57).contains(b[3]) { return false }
        return true
    }

    static func isFileName(_ s: String) -> Bool {
        guard let dot = s.lastIndex(of: ".") else { return false }
        let ext = s[s.index(after: dot)...]
        guard ["json", "png", "wav"].contains(ext) else { return false }
        return isName(String(s[..<dot]))
    }

    static func parseRev(_ s: Substring) -> UInt32? {
        guard !s.isEmpty, s.count <= 10, !s.hasPrefix("0"), s.allSatisfy({ $0.isASCII && $0.isNumber }) else { return nil }
        return UInt32(s)
    }

    /// An integer JSON number in 0...2^32-1 (not a Bool, no fraction).
    static func u32(_ v: Any?) -> UInt32? {
        guard let n = v as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID(), !CFNumberIsFloatType(n) else { return nil }
        let i = n.int64Value
        return (0...Int64(UInt32.max)).contains(i) ? UInt32(i) : nil
    }

    static func isHex64(_ v: Any?) -> Bool {
        guard let s = v as? String else { return false }
        return s.utf8.count == 64 && s.utf8.allSatisfy { ($0 >= 48 && $0 <= 57) || ($0 >= 97 && $0 <= 102) }
    }

    static func lessThanEll(_ s: ArraySlice<UInt8>) -> Bool {
        let b = Array(s)
        for i in stride(from: 31, through: 0, by: -1) where b[i] != ell[i] { return b[i] < ell[i] }
        return false
    }

    /// verify_strict: S < ℓ, R and A not of small order, then CryptoKit.
    static func strictVerify(_ a: [UInt8], _ message: [UInt8], _ sig: ArraySlice<UInt8>) -> Bool {
        let sig = Array(sig)
        guard sig.count == 64, lessThanEll(sig[32..<64]), !smallOrder.contains(Array(sig[0..<32])), !smallOrder.contains(a),
              let key = try? Curve25519.Signing.PublicKey(rawRepresentation: a) else { return false }
        return key.isValidSignature(sig, for: message)
    }

    static func le(_ b: [UInt8], _ at: Int, _ n: Int) -> Int {
        (0..<n).reduce(0) { $0 | Int(b[at + $1]) << (8 * $1) }
    }

    static func isRegularFile(_ url: URL) -> (Bool, Int) {
        guard let a = try? FileManager.default.attributesOfItem(atPath: url.path),
              a[.type] as? FileAttributeType == .typeRegular else { return (false, 0) }
        return (true, (a[.size] as? NSNumber)?.intValue ?? 0)
    }

    public func verifyInstalled(skinsDir: URL, customSkin: String, state: SkinState) -> Result<InstalledSkin, SkinCode> {
        // 1. Reference.
        guard let at = customSkin.firstIndex(of: "@") else { return .failure(.argument) }
        let id = String(customSkin[..<at])
        guard SkinSeal.isName(id), let rev = SkinSeal.parseRev(customSkin[customSkin.index(after: at)...]) else { return .failure(.argument) }
        // 2. Folder (lstat: a link is not a folder).
        let dir = skinsDir.appendingPathComponent(id).appendingPathComponent(String(rev))
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: dir.path), attrs[.type] as? FileAttributeType == .typeDirectory else {
            return .failure(.notInstalled)
        }
        // 3. seal.bin.
        let sealURL = dir.appendingPathComponent("seal.bin")
        let (regular, size) = SkinSeal.isRegularFile(sealURL)
        guard regular, size <= SkinSeal.maxSeal, let sealData = try? Data(contentsOf: sealURL), sealData.count <= SkinSeal.maxSeal else {
            return .failure(.tampered)
        }
        let b = [UInt8](sealData)
        // 4. Framing.
        guard b.count >= 12 else { return .failure(.malformed) }
        let magic = Array(b[0..<8])
        if magic == SkinSeal.recipientMagic { return .failure(.kind) }
        guard magic == SkinSeal.bundledMagic else { return .failure(.malformed) }
        let n = SkinSeal.le(b, 8, 4)
        guard (1...8192).contains(n) else { return .failure(.tooLarge) }
        guard b.count >= 12 + n + 2 else { return .failure(.malformed) }
        let c = SkinSeal.le(b, 12 + n, 2)
        guard (1...1024).contains(c) else { return .failure(.tooLarge) }
        guard b.count >= 142 + n + c else { return .failure(.malformed) }
        guard b.count == 142 + n + c else { return .failure(.malformed) }
        let headerBytes = Array(b[12..<12 + n])
        let certBytes = Array(b[14 + n..<14 + n + c])
        let certSig = b[14 + n + c..<78 + n + c]
        let headerSig = b[78 + n + c..<142 + n + c]
        // 5. Certificate.
        guard let cert = try? JSONSerialization.jsonObject(with: Data(certBytes)) as? [String: Any],
              Set(cert.keys) == ["issued", "kid", "pub", "root", "uses"],
              let kid = cert["kid"] as? String, SkinSeal.isName(kid),
              let rootKid = cert["root"] as? String, SkinSeal.isName(rootKid),
              SkinSeal.u32(cert["issued"]) != nil,
              let pubText = cert["pub"] as? String, let pubData = Data(base64Encoded: pubText), pubData.count == 32,
              let uses = cert["uses"] as? [String], [["code", "skin"], ["code"], ["skin"]].contains(uses) else {
            return .failure(.certificate)
        }
        if !acceptTestKeys && (rootKid.hasPrefix("test-") || kid.hasPrefix("test-")) { return .failure(.testKey) }
        guard rootKid.hasPrefix("test-") == kid.hasPrefix("test-"), let root = roots[rootKid] else { return .failure(.certificate) }
        guard SkinSeal.strictVerify(root, SkinSeal.certDomain + certBytes, certSig) else { return .failure(.certificate) }
        if state.revokedKids.contains(rootKid) || state.revokedKids.contains(kid) { return .failure(.revoked) }
        let pub = [UInt8](pubData)
        guard !SkinSeal.smallOrder.contains(pub) else { return .failure(.certificate) }
        // 6. Use.
        guard uses.contains("skin") else { return .failure(.keyUse) }
        // 7. Header signature, before reading the header.
        guard SkinSeal.strictVerify(pub, SkinSeal.bundledDomain + headerBytes, headerSig) else { return .failure(.signature) }
        // 8. Header.
        guard let h = try? JSONSerialization.jsonObject(with: Data(headerBytes)) as? [String: Any],
              Set(h.keys) == ["created", "files", "format", "id", "kid", "kind", "minClient", "payloadLen", "payloadSha256", "rev"],
              SkinSeal.u32(h["format"]) == 1, h["kind"] as? String == "bundled",
              let hid = h["id"] as? String, SkinSeal.isName(hid),
              let hrev = SkinSeal.u32(h["rev"]), hrev >= 1,
              let hkid = h["kid"] as? String, SkinSeal.isName(hkid),
              SkinSeal.u32(h["created"]) != nil,
              let mc = h["minClient"] as? [String: Any],
              mc.allSatisfy({ ["android", "ios", "macos", "windows"].contains($0.key) && $0.value is String }) else {
            return .failure(.header)
        }
        guard let payloadLen = SkinSeal.u32(h["payloadLen"]) else { return .failure(.header) }
        guard (8...4_199_160).contains(payloadLen) else { return .failure(.tooLarge) }
        guard SkinSeal.isHex64(h["payloadSha256"]), let list = h["files"] as? [[String: Any]], (1...48).contains(list.count) else {
            return .failure(.header)
        }
        var files: [(String, Int, String)] = []
        var total = 0
        for f in list {
            guard Set(f.keys) == ["len", "n", "sha256"], let name = f["n"] as? String, SkinSeal.isFileName(name),
                  !(name.hasSuffix(".json") && name != "manifest.json"), let len = SkinSeal.u32(f["len"]) else { return .failure(.header) }
            guard len <= 1_048_576 else { return .failure(.tooLarge) }
            total += Int(len)
            guard SkinSeal.isHex64(f["sha256"]) else { return .failure(.header) }
            if let last = files.last, Array(last.0.utf8).lexicographicallyPrecedes(Array(name.utf8)) == false { return .failure(.header) }
            files.append((name, Int(len), f["sha256"] as! String))
        }
        guard total <= 4_194_304 else { return .failure(.tooLarge) }
        guard files.contains(where: { $0.0 == "manifest.json" }) else { return .failure(.header) }
        guard hkid == kid else { return .failure(.header) }
        if state.revokedSkins.contains("\(hid)@\(hrev)") { return .failure(.revoked) }
        // 9. Path.
        guard hid == id, hrev == rev else { return .failure(.tampered) }
        // 10. Rollback.
        if rev < (state.highest[id] ?? 0) { return .failure(.downgrade) }
        // 11. Listing.
        guard let entries = try? FileManager.default.contentsOfDirectory(atPath: dir.path) else { return .failure(.tampered) }
        let allowed = Set(files.map { $0.0 }).union(["seal.bin"])
        for e in entries {
            guard allowed.contains(e), SkinSeal.isRegularFile(dir.appendingPathComponent(e)).0 else { return .failure(.tampered) }
        }
        guard entries.count == files.count + 1 else { return .failure(.tampered) }
        // 12. Files.
        var manifestData = Data()
        for (name, len, sha) in files {
            let url = dir.appendingPathComponent(name)
            let (reg, sz) = SkinSeal.isRegularFile(url)
            guard reg, sz <= len, let data = try? Data(contentsOf: url), data.count == len,
                  SkinSeal.hexString(Array(SHA256.hash(data: data))) == sha else { return .failure(.tampered) }
            if name == "manifest.json" { manifestData = data }
        }
        // 14. Serial.
        guard let manifest = try? JSONSerialization.jsonObject(with: manifestData) as? [String: Any] else { return .failure(.tampered) }
        if let serial = manifest["serial"] as? String, state.revokedSerials.contains(serial) { return .failure(.revoked) }
        return .success(InstalledSkin(id: id, rev: rev, dir: dir, manifest: manifest))
    }
}
