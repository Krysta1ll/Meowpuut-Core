# Custom skin test vectors

Shared vectors for every implementation of `plans/custom-skins.md` v2: the Rust core
(`crates/meowpuut-skin/tests/vectors.rs`), and later the Kotlin and CryptoKit load paths.
**Test keys only.** Every kid starts with `test-`; a release build refuses them all
(`MPS_E_TEST_KEY` = 6). The private keys come from public seeds, so anyone can sign with them:

| key | kid | seed (32 bytes) |
| --- | --- | --- |
| root | `test-root-1` | `SHA-256("meowpuut-skin test key test-root-1")` |
| subkey, uses `code`,`skin` | `test-2026-skin` | `SHA-256("meowpuut-skin test key test-2026-skin")` |
| request a / b (X25519) | — | `SHA-256("meowpuut-skin test key request a")` / `… request b` |

`cases.json` holds the keys (Base64), the subkey certificate, the redemption IDs, the client
(`android/1.2`) and the cases. Each case names a file, the request key to try, the policy
(`test`: test root trusted; `release`: nothing trusted, test kids refused), an optional state
(`state`) or revocation files to merge first (`revocations`), and the expected code (0 = opens).
For case `valid`, `installed` lists every file after the install-time rewrite with its length
and SHA-256: the same list as the signed header's `files`.

| file | what |
| --- | --- |
| `sakura_tea-3.mpskin` | the valid package for request a: two cats, a trim, an end piece, three sounds |
| `sakura_tea-3.seal.bin` | its `seal.bin` (magic, header, certificate, signature): the load-time check |
| `tampered-header.mpskin` | `"rev":3` changed to `"rev":4` after signing → 8 |
| `noncanonical-s.mpskin` | signature S replaced by S + ℓ → 8 (strict verification; CryptoKit must agree) |
| `small-order-eph.mpskin` | ephemeral key all zeros → 11 |
| `code-only-subkey.mpskin` | certificate with uses `["code"]` → 5 |
| `revoke-kid.mprevoke` | root-signed revocation of `test-2026-skin` |
| `revoke-by-subkey.mprevoke` | the same kind of file signed by the subkey → 28 |

The images are flat test shapes and the sounds Hann-windowed 1 kHz tones made by the test code.

## Formats in one place

All integers little endian. JSON that is signed or hashed is canonical: keys sorted by UTF-8
bytes, no white space, `serde_json` escaping and number form.

`.mpskin` v2:

```text
0        8   "MPSKIN" 00 02
8        4   header_len n (1..8192)
12       n   header JSON
12+n     2   cert_len c (1..1024)
14+n     c   certificate JSON {"issued","kid","pub","root","uses"}
14+n+c   64  root signature: Ed25519(root, "meowpuut-cert/v2\0" ‖ certificate)
78+n+c   64  header signature: Ed25519(subkey, "meowpuut-skin/v2\0" ‖ header)
142+n+c  m   payload: XChaCha20-Poly1305(CEK, nonce, MPAR, AAD = header without payloadSha256)
```

Header keys: `aead` (`xchacha20poly1305`), `created`, `files` (`[{len,n,sha256}]`, the rewritten
files, sorted), `format` (2), `id`, `kid`, `minClient` (`{"android":"1.2",…}`), `nonce` (24 bytes),
`payloadLen`, `payloadSha256` (of the ciphertext), `recipient` (`{code,eph,to,wrapped}`), `rev`.

Recipient and code (one request on one device):

```text
to        = SHA-256(request_pub)[0..16]
code      = 16 random bytes (the HKDF salt)
shared    = X25519(eph_secret, request_pub) = X25519(request_secret, eph)   must be contributory
content   = SHA-256(header without payloadSha256 and recipient)
KEK       = HKDF-SHA256(salt = code, ikm = shared,
                        info = "meowpuut-skin-kek/v2" ‖ u8 len(id) ‖ id ‖ u32 rev ‖ request_pub ‖ eph ‖ content)
wrapped   = XChaCha20-Poly1305(KEK, nonce = 24 zero bytes, CEK, AAD = "meowpuut-skin-cek/v2")   (48 bytes)
```

MPAR: `"MPAR"`, u16 version 1, u16 count (≤ 48); per entry u8 name length, name, u32 length
(≤ 1 MiB), SHA-256, data; names `[a-z0-9_-]{1,64}.(json|png|wav)`, sorted, no device names,
`manifest.json` required; files ≤ 4 MiB together.

`.mprevoke`: `"MPREVK"` 00 01, u32 body length, canonical body
`{"entries":[{"kind":"kid"|"skin"|"serial","value":…}],"root":"<root kid>","seq":<n>}`,
Ed25519(root, `"meowpuut-revoke/v1\0"` ‖ body).

Redemption ID: `"MPR2-"` + Crockford Base32 of `01 ‖ request_pub ‖ SHA-256("meowpuut-request/v1\0" ‖ 01 ‖ request_pub)[0..2]`
(56 symbols in groups of four).

## Bundled packages and installed seals (`bundled/`)

Bundled packages (`.mpskinb`) ship inside app updates (iOS and Android): signed by the same
root-certified `skin` subkeys, **not encrypted, not device-bound**. `bundled/index.json` indexes
them for the Swift/CryptoKit verifier (`crates/meowpuut-skin/SEAL.md`, which specifies `seal.bin`
and verify-on-load byte by byte) and for Kotlin:

* `packages`: install cases (`mps_skin_install_bundled`, client `ios/1.2`): file, policy, optional
  `state`, expected code. `packageFiles` says what each variant is.
* `installed`: the folder that installing `sakura_tea-3.mpskinb` writes (`installed/sakura_tea/3/`:
  the rewritten files and `seal.bin`, no `binding.bin`), its file list, and the seal's layout
  (offsets, parsed header and certificate, SHA-256 of the signed message).
* `loadCases`: copy `installed/` to a fresh skins folder, apply `ops` (flip, truncate, write,
  delete, move, symlink, copy a seal from `seals/`), verify `skin` with `state` under `policy`,
  expect a code. Covers changed, missing, extra and linked files, a planted `binding.bin`, a seal
  of another revision, a header changed after signing, S + ℓ, small-order R (CryptoKit alone
  accepts it), a recipient seal relabeled as bundled, a bundled header signed under the
  recipient domain, a subkey without the `skin` use, broken framing, revoked kid/root/skin/serial,
  rollback, and a malformed `customSkin`.
* `constants`: magics, signature domains, ℓ and the 14 small-order encodings.

```text
bundled .mpskinb:  "MPSKBN" 00 01 | u32 n | header | u16 c | cert | root sig | subkey sig | MPAR (plaintext)
header:            {"created","files","format":1,"id","kid","kind":"bundled","minClient","payloadLen","payloadSha256","rev"}
subkey signature:  Ed25519(subkey, "meowpuut-skin-bundled/v1\0" ‖ header), verify_strict
```

## Regenerate

Only after a deliberate format or encoder change (for example a new `png` version):

```sh
MEOWPUUT_WRITE_SKIN_VECTORS=1 cargo test --release -p meowpuut-skin --test vectors -- --ignored regenerate
MEOWPUUT_WRITE_SKIN_VECTORS=1 cargo test --release -p meowpuut-skin --test bundled_vectors -- --ignored regenerate
```

`vectors_are_current` and `bundled_vectors_are_current` fail until the committed files match what
the code builds.
