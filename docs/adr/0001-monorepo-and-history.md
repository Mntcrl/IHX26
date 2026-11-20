# ADR 0001 — Monorepo and imported iOS history

Status: accepted (2026-11-20)

## Context
- The Linux repository (`origin`, one commit `d2387a9 Initial commit`) held no code. The iOS app lived only on the Mac at `/Users/unity/Workspace/IXH26/app/XRPL_Galaxy` (branch `main`, 7 commits, no remotes), reachable from Linux as the `mac` remote (recon §1).
- The app repo contained a central backend (`backend/server.mjs`: challenge issuer, App Attest verifier, SQLite state, custodial wallet from `XRPL_TESTNET_SEED`), a POV committee client (coordinator at `http://10.164.228.104:8088`, Shamir/HPKE shares) and tests/docs tied to both (recon §6–§7). These contradict the target architecture: no project server, no coordinator, no custodial key.
- The user had already deleted `backend/`, `docs/` and `tests/` in the Mac working tree (17 uncommitted deletions; recon §1). That working tree must not be modified.
- The brief (§4) suggests `apps/`, `crates/`, `circuits/`, `bindings/`, `policies/`, `test-vectors/`, `fixtures/`, `scripts/`, `deployments/`, `docs/`.

## Decision
- Merge `mac/main` into this repository with `git merge --allow-unrelated-histories --no-ff` (merge commit `993c190`), so the 7 app commits keep their authorship and hashes. The `.gitignore` add/add conflict was resolved as the union of both files.
- Move the Xcode project unchanged (names kept) to `apps/capture-ios/`: `XRPL_Galaxy/`, `XRPL_Galaxy.xcodeproj/`, `AmbientConfiguration.plist`, `AppAttest.entitlements`, `Makefile`, `README.md`. The project uses a `PBXFileSystemSynchronizedRootGroup` on `XRPL_Galaxy/` with relative paths (recon §3), so the move needs no pbxproj edits.
- Remove `backend/`, `tests/`, `docs/` (legacy); their content remains in history. Replace `docs/` with the Galaxy docs tree. Keep a copy of the architecture reference at `docs/reference/XRPL_Galaxy_Architettura_v1.docx`.
- Lay out the monorepo per spec §1 (Rust workspace `crates/*`, `circuits/`, `apps/`, `bindings/`, …).
- Builds on the Mac use a separate mirror directory `/Users/unity/Workspace/IXH26/galaxy-build/` populated by `rsync` (see ADR 0008); the user's Mac working tree is never written to. Publishing to the Mac remote happens as a separate branch (`galaxy-v1`), never onto the checked-out `main` (`receive.denyCurrentBranch` default refuses it; recon §1).

## Consequences
- `git log` shows both histories joined by the merge commit; `git blame` on Swift files still reaches the original commits through the move (rename detection).
- The POV/backend Swift client code (`POVClient.swift`, `CaptureAttestationService.swift`, …) is still in `apps/capture-ios/` and is removed or rewritten by the IOS workstream.
- Anything that depended on repo-root paths (the app Makefile's `build/` output, docs links) now resolves relative to `apps/capture-ios/`.
