# PHØTON measurements

Runs use XRPL Testnet (`network_id = 1`) and a single-party development setup. The earlier desktop runs below use dev-unattested Ed25519 credentials, which cannot yield `VERIFIED`. The [phone run](#first-phone-certified-photo) uses Apple App Attest in the development environment.

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

## Final calibrated device capture — before AccountTxnID hardening

This historical run reused the initial DEVICE registration. It demonstrates the calibrated circuit and live proof flow, but its DEVICE and CAPTURE lack the `AccountTxnID` required by the current verifier.

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

## Device registration with AccountTxnID

Fresh faucet-funded development account `rnzVNEmtDoY4rHmbdnwzrsAzqqsccAJKfW` reused the already pinned final CIRCUIT above. Its DEVICE registration required **3 validated transactions / 30 drops** excluding the faucet funding payment. Publication plus readback was **32.789 s**. All three transactions validated with `tesSUCCESS`.

| Transaction | Ledger | MemoData bytes | Serialized Memos bytes | Signed blob bytes | AccountTxnID bytes | Fee (drops) |
|---|---:|---:|---:|---:|---:|---:|
| [AccountSet `SetFlag: 5`](https://testnet.xrpl.org/transactions/805D418D763EBF953DB5E21E446617E2B754D3D3DA32EC5D08AD67B7CBBB896A) | 22,489,687 | 0 | 0 | 152 | 0 | 10 |
| [DEVICE chunk](https://testnet.xrpl.org/transactions/C138C1D5D16EBD7DC026CB4F911536D19D5B84015C9E13069788F014DF19A937) | 22,489,690 | 245 | 302 | 481 | 33 | 10 |
| [DEVICE manifest](https://testnet.xrpl.org/transactions/7AED77759566E8FA3582433915F2568228F90F21EAF343ED0F591EF3AC7443D3) | 22,489,693 | 141 | 191 | 370 | 33 | 10 |

The chunk signed the zero sentinel and the manifest signed the chunk hash. Removing `AccountTxnID` and re-encoding measured 33 bytes of overhead per chained record (one field header byte plus 32 payload bytes); the memo sizes did not change.

## Real-device captures after AccountTxnID hardening

Both profiles used previously unused genuine-3D sensor captures, the new DEVICE above and the final accepted CIRCUIT. Each PROOF was checked locally before submission. Every CAPTURE and PROOF below validated with `tesSUCCESS`.

| Profile | CAPTURE | PROOF | `L0` | `Lsel` | `Lproof` | Fee (drops) |
|---|---|---|---:|---:|---:|---:|
| `png-v1` | [157D7D4E…](https://testnet.xrpl.org/transactions/157D7D4E04814A16319C2FB4815FB54D38CC3C8B92BCCFF60A589AED251B73B5) | [A10B1227…](https://testnet.xrpl.org/transactions/A10B1227F33E5CA6EBB01F79CF7C18CE5E544C757981377F7765F32BA7BFED12) | 22,489,766 | 22,489,771 | 22,489,776 | 20 |
| `jpeg-v1` | [B344A453…](https://testnet.xrpl.org/transactions/B344A45348DC7EC41BB7B0D60ED4923649B690CC8CF59D296C9177D3EC3BD891) | [AC7D58AA…](https://testnet.xrpl.org/transactions/AC7D58AAA5262A576B9B3E6AE50FB4CD0BBF4C6A4155B44ACB407CC2ADBD7CF5) | 22,489,787 | 22,489,792 | 22,489,798 | 20 |

CAPTURE: **366 B** MemoData, **418 B** serialized Memos, **597 B** signed blob. PROOF: **361 B** MemoData, **411 B** serialized Memos, **590 B** signed blob. Both included **33 B** of `AccountTxnID` and cost **10 drops** per transaction. Each PROOF signed its CAPTURE hash. With no intervening own transactions, the JPEG CAPTURE signed the PNG PROOF hash. XMP exports were 1,140,851 B (`png-v1`), 208,438 B (`jpeg-v1`), and 1,040,664 B for the lossless PNG export of the decoded JPEG raster.

| Timing / memory | `png-v1` | `jpeg-v1` |
|---|---:|---:|
| CAPTURE submit → observed validated | 7.800 s | 4.498 s |
| CAPTURE → selection, ledger close times | 19 s | 19 s |
| Selection → PROOF, ledger close times | 12 s | 12 s |
| PROOF submit → observed validated | 4.895 s | 6.113 s |
| Ingest and persist evidence | 127 ms | 147 ms |
| Commitments | 974 ms | 980 ms |
| Reference policy and build witness input | 107 ms | 95 ms |
| Desktop witness generation | 0.398 s | 0.517 s |
| Desktop Groth16 prove | 9.659 s | 9.693 s |
| Desktop prover self-check | 4.073 ms | 3.685 ms |
| Peak process RSS (`VmHWM`), entire capture command | 1,299,096 KiB | 876,784 KiB |
| Live shared verifier with RPC recording | 5.540 s | 5.325 s |
| Chromium browser UI, offline RPC replay | 1.261 s | 1.428 s |

Direct and manual `galaxy verify` returned **`UNSUPPORTED` (exit 60)** for both images: `registration.txn_chain`, `capture.txn_chain`, `history.complete`, both image bindings and `proof.groth16` passed. The image matrix also rejected substituted XMP, a modified pixel and JPEG recompression (`INVALID`); crops and resized images failed profile decoding (`UNSUPPORTED`). Both metadata-stripped rasters verified with explicit references. In Chromium, 31 replay RPCs per image produced the same full reports as the native CLI, apart from the RPC endpoint label. The [public RPC-only fixtures](../test-vectors/e2e/development-4ad25185-account-txn-id) contain 14 exchanges each (61,762 B PNG; 68,798 B JPEG); proof-only replays omit image binding.

Six distinct preceding CAPTUREs returned `INCONCLUSIVE` on their own ledger-derived challenges. Each cost 10 drops, published no PROOF, and was not re-challenged.

The local lookup node indexed Testnet ledgers 22,489,766–22,489,807. Live lookup returned `UNSUPPORTED` with all six checks above for both the stripped PNG and the original JPEG, and `NOT_FOUND` for an unregistered image within the node's coverage. Three independent SQLite indexes also recovered the JPEG binding and discovered both peers from one bootstrap each. A good index preserved the result alongside incorrect, omitting or failed peers; an omitting, failed or late index alone returned `UNAVAILABLE`. Direct XMP verification still passed after stopping every test node.

Before the history stress run, the JPEG verifier read one `account_tx` page in each window: 10 rows for CAPTURE, 1 for PROOF and 11 for REVOKE. It ignored no rows and did not walk the chain.

## Third-party history on Testnet (walk fallback)

A second funded development wallet sent **201 Payments** (10 drops each, ledgers 22,489,817–22,490,013; hashes in the private session's `payments.jsonl`, first [FBA99DD5…](https://testnet.xrpl.org/transactions/FBA99DD51B628015B254BE8EC430C2A115E8809C222ABA011AF4E84C771DBFCA)) to the registration account after the JPEG PROOF. `scripts/e2e-history.sh` then re-verified the JPEG with `--history-pages 1` (the verifier after the first hardening pass, three scans):

| Window | `account_tx` pages / rows | Non-records ignored | AccountTxnID steps |
|---|---:|---:|---:|
| A: CAPTURE | 1 / 10 | 0 | 0 |
| C: REVOKE | 1 / 200 | 189 | 12 |
| B: PROOF | 1 / 200 | 199 | 2 |

The verdict was unchanged (`UNSUPPORTED`, dev-unattested credential) with `history.complete`, `registration.txn_chain`, `capture.txn_chain`, `capture.unique` and `proof.groth16` passing: the third-party rows exhausted the one-page budget, the partial scans were discarded, and the verifier walked the account's own `AccountTxnID` chain instead. The second hardening pass (ADR 0093) merges B and C into one scan; on the offline padded recording it reduced the RPC calls from 218 to 144.

## First phone-certified photo

The PHØTON iPhone app's third attempt produced a certified `jpeg-v1` directly on the phone. The phone's [DEVICE manifest](https://testnet.xrpl.org/transactions/76D0884DDC2129FCE8D906B92468943D12377418711B5A111A07C2379FAC7A91) validated at ledger 22,493,823. [CAPTURE `7DEA7F30…`](https://testnet.xrpl.org/transactions/7DEA7F30DBCEBE5461CDC1E57B2E3E66A4F5617D6708F6B0250796B3BF4C369D) validated at 22,494,316, selection was at 22,494,321, and [PROOF `F50D1D20…`](https://testnet.xrpl.org/transactions/F50D1D20B1B784657B941CD291B6EB44B9B332AF63D8A1356C144C94C69C7800) validated at 22,494,467. The PROOF signed the CAPTURE's hash as its `AccountTxnID`. The verifier accepted Apple App Attest in the development environment and the [final calibrated circuit](#testnet-registration--final-calibrated-artifact-set).

| First phone photo, from validated Testnet records | MemoData | Signed blob | Fee |
|---|---:|---:|---:|
| CAPTURE (App Attest) | 442 B | 673 B | 10 drops |
| PROOF | 361 B | 590 B | 10 drops |

Sizes are from the XRPL `tx` responses: decoded hexadecimal `MemoData` and binary signed transaction. The validated close times were 12 s apart from CAPTURE to selection and 502 s apart from CAPTURE to PROOF for this first capture. These are ledger intervals, not handset timings.

The native report summary was “Physical consistency verified under Galaxy policy galaxy-pc-v1m (4ad25185f11bfb52af5dee5a1ab39d4e2428c1c83188c7483db1777d22367073).” Direct verification passed 39 checks:

- `image.{decode,profile,photo_id,rgb_root}` and `refs.metadata`.
- `ledger.{network,preledger_window,selection,challenge}`.
- `capture.{found,validated_success,decode,network,txn_chain,photo_binding,rgb_binding,account_binding,assertion,counter,unique}`.
- `circuit.{accepted,registration}`, `registration.{resolve,txn_chain,bundle,attestation,revocation}`, and `history.complete`.
- `proof.{found,validated_success,account,binding,window,groth16}` and `policy.{sensor_quality,lidar_geometry,rgb_lidar,temporal,motion_imu}`.

`evidence.bluetooth` and `evidence.noise` were marked not applicable as proof checks; these signals were committed, not proven.

| Verification path | Result | Wall time |
|---|---|---:|
| Linux CLI, original JPEG, live XRPL | `VERIFIED` (exit 0) | 6.271 s |
| Linux CLI, XMP removed, explicit CAPTURE and PROOF references | `VERIFIED` (exit 0) | 6.195 s |
| Linux CLI, XMP removed, loopback node lookup and live XRPL | `VERIFIED` (exit 0) | 6.284 s |
| Linux CLI, unmodified canonical raster repacked losslessly as PNG with original XMP | `VERIFIED` (exit 0) | 6.980 s |
| Linux CLI, same PNG with one pixel's red channel changed by one bit | `INVALID` (exit 20), `capture.photo_binding` failed | 2.298 s |
| Linux Chromium, original JPEG, default live XRPL | `VERIFIED` | 6.337 s |
| Linux Chromium, original JPEG, replay of the CLI's XRPL responses | `VERIFIED` | 0.997 s |
| macOS arm64 CLI, original JPEG, live XRPL | `VERIFIED` (exit 0) | 8.086 s |
| macOS Chrome, original JPEG, default live XRPL | `VERIFIED` | 6.153 s |

The local node took 149.287 s to index the contiguous range 22,494,316–22,494,467. This was a deliberately bounded index, not complete Testnet history. Stripping XMP left the canonical raster unchanged. Linux Chromium's full report on the 13 replayed RPC calls matched the native report except for the RPC endpoint label. Both live browsers and the macOS CLI agreed with the Linux CLI on the verdict, the image binding, transaction references, and every check status; the live runs consulted different validated tips.

## On-device proving and capture timings

The [device acceptance run](PROGRESS.md#acceptance-criteria-brief-25) recorded these ranges on iPhone 13 Pro Max; the shutter-to-certified range is not a timing for the first historical transaction pair above.

| Quantity | Measured |
|---|---:|
| Groth16 circuit constraints | 876,639 |
| Witness generation | 0.27–0.38 s |
| Groth16 proving | 6.2–9.1 s |
| Peak physical memory footprint | ~1.0 GiB |
| Shutter to certified | 35–46 s |
