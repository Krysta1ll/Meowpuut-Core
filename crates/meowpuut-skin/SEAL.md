# `seal.bin` and verify-on-load, for a verifier that does not link Rust

This is the exact contract of `meowpuut_skin::install::verify_installed` (Rust), written so the
iOS keyboard can implement it with CryptoKit (`Curve25519.Signing`, `SHA256`) and Foundation only,
and reach the same verdict and error code on every input. The golden vectors are in
`Tests/skin-vectors/bundled/` (`index.json`); a verifier is correct when every `loadCases` entry
gives its `expect`.

`reference/SkinSealVerifier.swift` implements §5 for bundled seals with CryptoKit and Foundation;
`reference/main.swift` runs the vectors' `loadCases` through it (all 27 agree with Rust):

```sh
cd crates/meowpuut-skin/reference
swiftc -O SkinSealVerifier.swift main.swift -o /tmp/skin-seal && /tmp/skin-seal ../../../Tests/skin-vectors/bundled
```

iOS installs only **bundled** skins (the app bundles signed `.mpskinb` packages; redemption is
Android only), so the iOS keyboard should accept only the bundled magic and refuse a recipient
seal with code 32 (`Kind`). Rust's own loader accepts both kinds; the recipient-only steps are
marked (R) below for completeness.

Everything is little endian. Error codes are `meowpuut_skin::Code` / `MPS_E_*` in
`crates/meowpuut-ffi/include/meowpuut.h`. "Refuse with N" means: stop, show the built-in skin
(`base` or `current`), read no image or sound.

## 1. Inputs

| input | where it comes from |
| --- | --- |
| `skinsDir` | the App Group folder `Skins/` |
| `customSkin` | the `customSkin` setting, `"<id>@<rev>"` (untrusted text) |
| `roots` | the pinned root public keys: `kid → 32 bytes`. Release: `RELEASE_ROOTS` of `src/trust.rs` (empty until the developer generates the real root, so a Release build refuses every skin until then). Debug/Lab only: also `test-root-1` (`index.json` `keys.root.public`). |
| `acceptTestKeys` | true only in Debug/Lab. A Release build must refuse every kid starting with `test-` and must not contain the test root (fail the build otherwise). |
| `state` | the device's monotonic state JSON (Keychain, shared access group), `{"highest":{"<id>":<rev>},"revoked":{"kid":[…],"serial":[…],"skin":[…]},"seq":<n>,"v":1}`; empty means none. Union it with the built-in revocations of the app (`BUILTIN` of `src/revoke.rs`). |

## 2. Encodings

* **Names** (`is_name`): 1–64 bytes of `[a-z0-9_-]`, and not a Windows device name: `con`,
  `prn`, `aux`, `nul`, `com1`–`com9`, `lpt1`–`lpt9`. Skin IDs, kids and file stems.
* **File names**: `<stem>.<ext>`, stem a name, ext `json`, `png` or `wav`; only `manifest.json`
  may be JSON.
* **Revisions**: decimal `u32` ≥ 1, no sign, no leading zero, at most 10 digits.
* **Base64**: RFC 4648 standard alphabet, padded; only the spelling an encoder produces (no
  whitespace, zero padding bits).
* **Hex**: lowercase only.
* **JSON** that is signed is canonical: object keys sorted by UTF-8 bytes, no whitespace,
  `serde_json` string escaping, integers without fraction or exponent. Rust refuses a signed
  text that is not canonical (`Header`/`Certificate`). A Swift verifier may parse with
  `JSONSerialization` without re-checking canonical form, because the bytes are signed and the
  developer tools only ever emit canonical JSON: a difference here is reachable only by the
  holder of a signing key. It must still check every key set exactly and every type and range
  below.

## 3. `seal.bin`

`seal.bin` is the package from its first byte up to and including the header signature,
unchanged. Total length ≤ `maxSeal` = 9358 bytes.

```text
offset    size  field
0         8     magic: 4d 50 53 4b 42 4e 00 01  "MPSKBN\0\x01"  bundled
                       4d 50 53 4b 49 4e 00 02  "MPSKIN\0\x02"  recipient (R)
8         4     n = header length, u32         1 ≤ n ≤ 8192
12        n     header JSON
12+n      2     c = certificate length, u16    1 ≤ c ≤ 1024
14+n      c     certificate JSON
14+n+c    64    root signature of the certificate
78+n+c    64    subkey signature of the header
142+n+c         end of file (exactly)
```

Signed messages (no separator other than the trailing NUL of each domain string):

| what | message | key |
| --- | --- | --- |
| certificate | `"meowpuut-cert/v2\0"` ‖ certificate bytes | the root named by the certificate's `root` |
| bundled header | `"meowpuut-skin-bundled/v1\0"` ‖ header bytes | the certified subkey |
| recipient header (R) | `"meowpuut-skin/v2\0"` ‖ header bytes | the certified subkey |

The magic selects the domain, the domain separates the kinds: a recipient header (signed under
`meowpuut-skin/v2\0`) never verifies as bundled, whatever magic is put in front of it.

### Certificate JSON (exactly these five keys)

```text
{"issued":<u32>,"kid":"<name>","pub":"<Base64 of 32 bytes>","root":"<name>","uses":["code"|"skin",…]}
```

`uses` is non-empty, strictly ascending (`["code","skin"]`, `["code"]` or `["skin"]`).

### Bundled header JSON (exactly these ten keys)

```text
{"created":<u32>,
 "files":[{"len":<u32>,"n":"<file name>","sha256":"<64 hex>"},…],
 "format":1,"id":"<name>","kid":"<name>","kind":"bundled",
 "minClient":{"<platform>":"<major>.<minor>",…},
 "payloadLen":<u32>,"payloadSha256":"<64 hex>","rev":<u32 ≥ 1>}
```

* `files`: 1–48 entries, strictly ascending by `n` (byte order, so unique), `manifest.json`
  present, each `len` ≤ 1048576, the sum ≤ 4194304. These are the **installed** files (after
  the install-time rewrite) — the files verify-on-load hashes.
* `minClient` keys among `android`, `ios`, `macos`, `windows`; values `major.minor`, each part
  1–6 digits without a leading zero (except `0`). Not checked on load (install checks it).
* `payloadLen` 8–4199160, `payloadSha256` of the plaintext MPAR archive: install-time only.
* `created` is informational. No time is used for any decision.

A recipient header (R) instead has exactly `aead` (`"xchacha20poly1305"`), `created`, `files`,
`format` (`2`), `id`, `kid`, `minClient`, `nonce`, `payloadLen`, `payloadSha256`, `recipient`
(`{"code","eph","to","wrapped"}`), `rev`: see `src/container.rs`.

## 4. Ed25519: matching `verify_strict`

Rust verifies every signature with ed25519-dalek 3.0.0 `VerifyingKey::verify_strict`:

1. the public key decodes (dalek `decompress`: the low 255 bits as y, reduced mod p, any y with a
   square root, the top bit picks the sign of x — **non-canonical y and "negative zero" x are
   accepted**);
2. `S` (bytes 32–63, little endian) is **< ℓ** = 2^252 + 27742317777372353535851937790883648493
   (`index.json` `constants.ell`, little endian);
3. `R` (bytes 0–31) decodes, and **neither `R` nor the public key is of small order**;
4. `k = SHA-512(R bytes ‖ public key bytes ‖ message)` mod ℓ, and the canonical encoding of
   `[S]B − [k]A` **equals the 32 bytes of `R`** (cofactorless, byte comparison).

Roots and certified subkeys are additionally refused when weak (`is_weak`: small order).

CryptoKit (`Curve25519.Signing.PublicKey.isValidSignature(_:for:)`), measured on this machine
(macOS, Swift 6.2) against the vectors:

| input | CryptoKit | verify_strict | what a Swift verifier must do |
| --- | --- | --- | --- |
| valid signature | true | ok | — |
| `S + ℓ` (`seals/noncanonical-s.seal.bin`) | false | refused | check `S < ℓ` itself anyway (cheap, and not documented by Apple) |
| `R = 01 00…00` (identity), `S = k·a` (`seals/small-order-r.seal.bin`) | **true** | refused | **refuse when `R` is one of the 14 small-order encodings** |
| public key `02 00…00` (y = 2 is not on the curve) | `init(rawRepresentation:)` **does not throw** | `from_bytes` fails | refuse a subkey whose key is a small-order encoding (CryptoKit's init checks nothing); an off-curve subkey can only come from the root's holder |

So a Swift verifier calls CryptoKit only after these byte checks, and the verdicts then agree:

```text
strictVerify(publicKey A, message, signature):
    R = signature[0..<32], S = signature[32..<64]
    if S >= ℓ (compare as 256-bit little-endian integers)  → false
    if R ∈ SMALL_ORDER or A ∈ SMALL_ORDER                  → false
    return CryptoKit.Curve25519.Signing.PublicKey(rawRepresentation: A).isValidSignature(signature, for: message)
```

`SMALL_ORDER` is the 14 encodings that dalek decompresses to a point of small order
(`index.json` `constants.smallOrderEncodings`; `tests/bundled.rs` checks each is weak in dalek):

```text
0000000000000000000000000000000000000000000000000000000000000000   y = 0
0000000000000000000000000000000000000000000000000000000000000080
0100000000000000000000000000000000000000000000000000000000000000   y = 1 (identity)
0100000000000000000000000000000000000000000000000000000000000080
ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f   y = p − 1
ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f   y = p (≡ 0)
edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f   y = p + 1 (≡ 1)
eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05   order 8
26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85
c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a   order 8
c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa
```

Why the list is complete: the 8 small-order points have y ∈ {0, 1, p−1, y₈, p−y₈}; an encoding's
low 255 bits may be y or y + p when that is < 2^255 (only for y = 0 and y = 1), and either sign
bit decodes (dalek negates x = 0 to itself).

Remaining differences, all reachable only by the holder of a private key (a forger cannot make a
valid signature at all): a non-canonical `R` that is not of small order (both compare bytes
against a canonical encoding, so both refuse); a certified subkey with a non-canonical but
decodable encoding (the developer tools never make one; CryptoKit may differ); non-canonical
JSON or Base64 in signed text (§2).

## 5. Verify-on-load, step by step

Rust order, which fixes the error code when several things are wrong. `fail(N)` = refuse with N.

1. **Skin reference.** Split `customSkin` at the first `@`: `id` must be a name, `rev` a
   revision (§2), else `fail(26 Argument)`. Never build a path from the raw text.
2. **Folder.** `dir = skinsDir/<id>/<decimal rev>`. If `dir` is not a directory (`lstat`; a
   symbolic link is not one) → `fail(23 NotInstalled)`.
3. **Read `seal.bin`.** `lstat(dir/seal.bin)`: missing or not a regular file (a symbolic link is
   not) → `fail(24 Tampered)`; size > 9358 → `fail(24)`. Read it.
4. **Framing** (§3), checking each length before slicing:
   size < 12 → `fail(2 Malformed)`; magic neither bundled nor recipient → `fail(2)`;
   (iOS: recipient magic → `fail(32 Kind)`);
   `n` = 0 or > 8192 → `fail(1 TooLarge)`; fewer than `12 + n + 2` bytes → `fail(2)`;
   `c` = 0 or > 1024 → `fail(1)`; fewer than `142 + n + c` bytes → `fail(2)`;
   more than `142 + n + c` bytes → `fail(2)`.
5. **Certificate** → any failure here is `fail(4 Certificate)` unless stated:
   1. parse: an object with exactly `issued`, `kid`, `pub`, `root`, `uses`; `kid` and `root`
      names; `issued` an integer 0…2^32−1; `pub` Base64 of exactly 32 bytes; `uses` as in §3.
   2. if `root` or `kid` starts with `test-` and `!acceptTestKeys` → `fail(6 TestKey)`
      (the root first, then the kid).
   3. `test-` root with a non-`test-` kid, or the reverse → `fail(4)`.
   4. `root` not in `roots` → `fail(4)`.
   5. `strictVerify(roots[root], "meowpuut-cert/v2\0" ‖ cert bytes, root signature)` false →
      `fail(4)`.
   6. `root` or `kid` in `state.revoked.kid` → `fail(7 Revoked)`.
   7. `pub` ∈ `SMALL_ORDER` → `fail(4)`. (Rust also refuses a `pub` that is not a curve point;
      CryptoKit cannot tell, and only the root's holder could certify one.)
6. **Use.** `uses` lacks `"skin"` → `fail(5 KeyUse)`.
7. **Header signature.** `strictVerify(pub, domain(magic) ‖ header bytes, header signature)`
   false → `fail(8 Signature)`. Nothing in the header is read before this.
8. **Header** (kind from the magic) → `fail(3 Header)` unless stated: exactly the keys of §3;
   `format` 1 and `kind` `"bundled"` (R: `format` 2, `aead`); `id`, `kid` names; `rev` 1…2^32−1;
   `created` 0…2^32−1; `minClient` as in §3; `payloadLen` an integer 0…2^32−1 and in range
   (bundled 8…4199160, R 16…4199176) else `fail(1)`; `payloadSha256` 64 lowercase hex; `files`
   as in §3 (a `len` over 1 MiB or a sum over 4 MiB → `fail(1)`); (R: `nonce`, `recipient`).
   Then: header `kid` ≠ certificate `kid` → `fail(3)`; `"<id>@<rev>"` of the header in
   `state.revoked.skin` → `fail(7)`.
9. **Path.** header `id` ≠ `id` or header `rev` ≠ `rev` → `fail(24)`.
10. **Rollback.** `rev < state.highest[id]` (0 when absent) → `fail(18 Downgrade)`.
11. **Listing.** Every entry of `dir` must be a regular file (not a link, not a folder) named
    `seal.bin`, one of the header's `files`, or (R only) `binding.bin`; and the count must be
    exactly `files.count + 1` (R: `+ 2`). Otherwise `fail(24)`.
12. **Files**, in header order: `lstat` must be a regular file of size ≤ `len`, read it, its
    size must equal `len` and its SHA-256 equal `sha256`; otherwise `fail(24)`.
13. (R) **Binding.** `binding.bin` (≤ 1024 bytes) must equal the canonical JSON of the header's
    `recipient`, else `fail(24)`; and when the platform can read its request public keys, one
    of them must satisfy `SHA-256(pub)[0..16] == recipient.to`, else `fail(25 Binding)`.
14. **Serial.** Parse `manifest.json` (already hash-checked); its `serial` (16 lowercase hex)
    in `state.revoked.serial` → `fail(7)`.
15. Accept. Use `manifest.json`'s `base`, `names`, `palette`, `glass`, `keys`, `cats`,
    `sounds` (format: `src/manifest.rs`); read images and sounds only from `dir`, only names
    listed in `files`.

Reading the whole folder costs one SHA-256 pass over ≤ 4 MiB plus three Ed25519 checks (two for
the certificate chain and header, none per file).

## 6. Installing (Rust only)

The containing app installs with `mps_skin_install_bundled(package, len, state_json, skins_dir,
"ios/<major>.<minor>", out, cap)`: it runs all of §5's seal checks on the package, then the
client version (`minClient`, `20 Client`), the rollback guard (`18`), the payload length and
SHA-256 (`9 Payload`), the MPAR archive (`13`), the validator (`14`/`15`/`16`), the serial (`7`)
and the comparison of the rewritten files with `files` (`17`); writes `<id>/<rev>/` with the
files and `seal.bin` (no `binding.bin`) into a temporary folder, verifies that folder exactly as
§5, renames it into place and removes other revisions of that ID. It returns the new state
(`"state"`) to store in the Keychain; never lower `highest` or drop revocations. A recipient
package fed to it fails with `32 Kind`.

## 7. Package layout (bundled `.mpskinb`)

```text
seal (§3, bundled magic) ‖ payload
payload = MPAR archive, plaintext, exactly payloadLen bytes, SHA-256 = payloadSha256:
  "MPAR" u16 1 u16 count(1–48)
  per entry: u8 name length, name, u32 length (≤ 1 MiB), 32-byte SHA-256 of the data, data
```

The archive holds the files **before** the rewrite (the app re-encodes every PNG and WAV and
re-serialises the manifest, then compares with `files`), so the package and the installed
folder differ in content but the installed bytes are exactly what the header signs.
