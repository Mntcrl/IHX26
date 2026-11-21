# XRPL Galaxy development measurements

Linux desktop, XRPL Testnet (`network_id = 1`). These runs use a development-single-party circuit setup and a development-unattested Ed25519 credential. They are not evidence of Apple App Attest, on-device proving, or an end-to-end `VERIFIED` capture.

## Testnet registration — initial artifact set

- CIRCUIT manifest: [`1C83F97F898195DD1AE71CC13EAD0435C187248A9B79C22B1786675F36995A59`](https://testnet.xrpl.org/transactions/1C83F97F898195DD1AE71CC13EAD0435C187248A9B79C22B1786675F36995A59), circuit `bfdc24d683e594640a3059ac83f4717fdedcfaef7f061a9093b783aada3b08ea`, policy `ef744a7f28dc606b117dbab34899753a218cd1865b4fc69a9739ac9c10a7afac` (3 mm per-side motion baseline; 876,639 constraints), VK `3150a6a6540b384b141bfd11738d954e22b8e82b8cb145de1abfee19b4e81b74`.
- DEVICE manifest: [`B9A9870609B314F66AE80584F34343AE6D09279438AFE77897DB21A5A3F8E259`](https://testnet.xrpl.org/transactions/B9A9870609B314F66AE80584F34343AE6D09279438AFE77897DB21A5A3F8E259), development-unattested.

| Transaction | Number | MemoData bytes each | Serialized Memos field bytes each | Signed blob bytes each | Fee each (drops) |
|---|---:|---:|---:|---:|---:|
| CIRCUIT chunk (full) | 5 | 839 | 897 | 1,043 | 10 |
| CIRCUIT chunk (last) | 1 | 323 | 381 | 527 | 10 |
| CIRCUIT manifest | 1 | 147 | 198 | 344 | 10 |
| DEVICE chunk | 1 | 245 | 302 | 448 | 10 |
| DEVICE manifest | 1 | 141 | 191 | 337 | 10 |

Circuit registration used 7 transactions / 70 drops, validated in ledgers 22,483,664–22,483,681. DEVICE registration used 2 transactions / 20 drops, validated in ledgers 22,483,686–22,483,689. The serial `galaxy-dev` publish-and-retrieve steps took 70.914 s and 24.798 s respectively, including network RPC, validation polling, and reading back the exact published bundles. Each transaction validated with `tesSUCCESS`, and all fees were below the configured 1,000-drop maximum. Memos field sizes are the difference in encoded transaction size with and without `Memos`, not JSON/string lengths. Registration is separate from per-photo costs.

## Real-device capture replay — initial artifact set

Private iPhone `genuine-3d` GXRW captures were ingested on Linux. Each attempt used its own sensor capture, committed before the Testnet challenge was known. Linux replay time in CAPTURE is declarative, not the original iPhone capture time.

| Profile | CAPTURE ledger (`L0`) | Selection (`L0 + 5`) | PROOF ledger | Transactions per photo | Fee per photo (drops) |
|---|---:|---:|---:|---:|---:|
| `png-v1` | 22,483,775 | 22,483,780 | 22,483,787 | 2 | 20 |
| `jpeg-v1` | 22,483,819 | 22,483,824 | 22,483,831 | 2 | 20 |

| Transaction | MemoData bytes | Serialized Memos field bytes | Signed blob bytes | Fee (drops) |
|---|---:|---:|---:|---:|
| CAPTURE | 366 | 418 | 564 | 10 |
| PROOF | 361 | 411 | 557 | 10 |

| Timing | `png-v1` | `jpeg-v1` |
|---|---:|---:|
| CAPTURE submit → observed validated | 5.283 s | 6.003 s |
| CAPTURE → selection, ledger close times | 12 s | 19 s |
| Selection → PROOF, ledger close times | 28 s | 21 s |
| PROOF submit → observed validated | 7.431 s | 7.507 s |
| Ingest and persist evidence | 204 ms | 176 ms |
| Commitments | 1,135 ms | 1,068 ms |
| Reference policy and build witness input | 143 ms | 102 ms |
| Desktop witness generation | 0.616 s | 0.408 s |
| Desktop Groth16 prove | 13.013 s | 12.626 s |
| Desktop prover self-check | 4.234 ms | 3.000 ms |
| Live CLI verification | 9.587 s | 9.599 s |
| Offline CLI verification | 0.387 s | 0.366 s |
| Chromium browser UI, offline RPC replay | 2.159 s | 2.223 s |

`png-v1` references: [CAPTURE](https://testnet.xrpl.org/transactions/CEE493CAD78900FEE158EBC35C659C00DEB14647CB230038F8134D3EEC5E5707), [PROOF](https://testnet.xrpl.org/transactions/298614A7A33605F9125DF05E0A2868FFFD7A196B51613A27F3A15CDBD9921BAE). `galaxy verify --no-discovery` returned `UNSUPPORTED` (exit 60). Both image bindings, the future-ledger checks, and Groth16 passed. The only failed check was App Attest by design; the dev counter and committed-only evidence checks do not apply. The verifier's PNG recording contains 12 read-only exchanges (35,139 bytes). It replays with or without the image (proof-only mode does not check the image binding).

`jpeg-v1` references: [CAPTURE](https://testnet.xrpl.org/transactions/D3E7C2F99665F964753A97EB0A73F8913F532388244655045626EA62F3E62364), [PROOF](https://testnet.xrpl.org/transactions/4B219E8A92F7FD96A3226C0D17B819520F8608918419FBBD152B44AA408ED980). The JPEG and its pixel-identical PNG export gave the same result with the same transaction references. The JPEG recording contains 12 exchanges (39,817 bytes). Both profiles have public RPC-only fixtures in [`test-vectors/e2e/development-ef744a7f`](../test-vectors/e2e/development-ef744a7f); private exports are not included.

Two other real captures were `INCONCLUSIVE` under their own ledger-derived challenges and no PROOF was published for either: [PNG attempt](https://testnet.xrpl.org/transactions/56ACBB3DE43EC2CF70AAEAFB17952FC5528206DFCD368ADAC15F24B37451D793) (`Lsel` 22,483,769) and [JPEG attempt](https://testnet.xrpl.org/transactions/3F78668E35AD3CD879BEE2C18B1D10A6D93AA98453B107EEABEFB2434AB74BE0) (`Lsel` 22,483,813). Each cost one CAPTURE fee and was replaced with a different capture, not a new challenge.

The replay matrix returned `INVALID` for substituted XMP, one changed RGB channel, and JPEG recompression with the original XMP. Cropping and resizing returned `UNSUPPORTED` at image-profile decoding (dimensions), not at the development credential. Stripped PNG XMP was recoverable using manual references and via a real lookup node, which indexed Testnet ledgers 22,483,775–22,483,871. The same node returned no candidates for an unregistered image; `galaxy verify --lookup` reported `NOT_FOUND`, limited to that coverage.

Headless Chromium processed both private images using the production browser verifier build and a loopback replay of public XRPL results. From the Verify click to the displayed report, it made 27 RPC calls per image (12 unique entries); each report matched the native CLI report after normalizing the RPC endpoint label.

A separate real `account_tx` request through the PNG PROOF covered ledgers 22,483,664–22,483,800. The endpoint returned 10 results and a marker on page 1, then 2 results and no marker on page 2. The narrower CAPTURE-to-PROOF query returned both in a single page. The server clamps the low limit even when the client requests one result.

Interrupted CAPTURE recovery was demonstrated on Testnet: the resumed run recovered the exact original signed [CAPTURE](https://testnet.xrpl.org/transactions/D19B37C740967EFBF89B2F933F7591A9D74F673080E3DE5E1E324B501D9C3CE7) by hash and published a validated [PROOF](https://testnet.xrpl.org/transactions/C36055B7EC02DFE7BC1D99EF928F5611294C6B7749EBDA677D97CAA778AEF442); the journal reported two transactions, not a replacement CAPTURE.

Three independent `galaxy-node` processes reconstructed the JPEG's Testnet range (22,483,819–22,483,832) in separate SQLite caches. Starting with one bootstrap endpoint each, all three discovered both other signed peers through peer exchange. CLI lookup through each independent index recovered the stripped image's proof and reported `UNSUPPORTED` with Groth16 passing.

Pairing the good index with a node that returned incorrect references, omitted the capture, or failed lookup left the result unchanged. An omitting, failed, or late-starting index on its own returned `UNAVAILABLE` in this deliberately bounded scan. After stopping all test nodes, direct XMP verification still passed Groth16 and returned `UNSUPPORTED`.

## Testnet registration — final calibrated artifact set

The final 876,639-constraint development circuit `a0bed7f1cdad94c33f9ff22e8b6cdd1bdb7e7f873ce73c8038e8a403213fa68b` pins policy `4ad25185f11bfb52af5dee5a1ab39d4e2428c1c83188c7483db1777d22367073` and VK `7cf9640233d89ee56677b2737f846cb9af215faaabe9fe8fd05062df5463a908`. Its [CIRCUIT manifest](https://testnet.xrpl.org/transactions/B0C14DC5C6BEC26A5C43CD236383EC1893F164DF0047498ACF38F67C4DC75D66) validated at ledger 22,486,132 and is the only entry in the shared Testnet allowlist.

| Transaction | Number | MemoData bytes each | Serialized Memos field bytes each | Signed blob bytes each | Fee each (drops) |
|---|---:|---:|---:|---:|---:|
| CIRCUIT chunk (full) | 5 | 839 | 897 | 1,043 | 10 |
| CIRCUIT chunk (last) | 1 | 322 | 380 | 526 | 10 |
| CIRCUIT manifest | 1 | 147 | 198 | 344 | 10 |

All 7 transactions validated with `tesSUCCESS` in ledgers 22,486,119–22,486,132. Publication and historical readback of the 4,283-byte GXCB took 59.105 s and cost 70 drops.

## Final calibrated device captures — before DepositAuth hardening

This run reused the historical development DEVICE registration above. It is evidence of the calibrated circuit and live proof flow, not evidence that the registration passes the newer DepositAuth checks. Re-register the device with the hardened publisher before testing the updated verifier.

| Profile | `L0` | `Lsel` | `Lproof` | Transactions per photo | Fee (drops) |
|---|---:|---:|---:|---:|---:|
| `png-v1` | 22,486,144 | 22,486,149 | 22,486,154 | 2 | 20 |

| Timing / memory | `png-v1` |
|---|---:|
| CAPTURE submit → observed validated | 4.395 s |
| CAPTURE → selection, ledger close times | 20 s |
| Selection → PROOF, ledger close times | 12 s |
| PROOF submit → observed validated | 4.849 s |
| Desktop witness generation | 0.424 s |
| Desktop Groth16 prove | 9.600 s |
| Peak process RSS (Linux `VmHWM`, entire capture command) | 1,398,048 KiB |
| Live shared verifier with RPC recording | 7.738 s |

PNG [CAPTURE](https://testnet.xrpl.org/transactions/3DB9B6D178054422A2477E73AEF2F21E2610EF5AFBD2A7CE94DFD3182B94A971) and [PROOF](https://testnet.xrpl.org/transactions/4F1DEC5B73FB09938626B6AE0760728DD061E17A84E0F9C6945476F4F2C97434) both validated. Image binding and Groth16 passed; `galaxy verify` returned `UNSUPPORTED` for the dev-unattested credential. The PNG recording contains 12 exchanges (58,189 bytes).
