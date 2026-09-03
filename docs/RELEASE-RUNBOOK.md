# Release Runbook

This document records the code-signing decision for Corvus Capture releases and the
post-publish procedures that must be completed before a release is announced.

## Warning: git history rewrites

If a future release (or any other task) requires rewriting git history (`git filter-branch`,
`git filter-repo`, BFG, etc.) — for example to purge a file that was force-tracked by
mistake — do **not** proceed directly to dropping backup refs or running an aggressive gc.
During v0.1.0's release hardening, running `git filter-branch` followed immediately by
dropping `refs/original/` and `git gc --prune=now --aggressive`, *before* checking the
working tree, permanently deleted 21 historical planning documents: `filter-branch`'s final
step is a `git checkout -f` of the rewritten HEAD, which also removes any working-tree files
that were purged from the rewritten history, not just their tracked/history copies. Before
running `gc --aggressive` or deleting `refs/original/` after any history rewrite:

1. Create a backup branch or tag of the pre-rewrite tip (e.g. `git branch backup-pre-rewrite <old-sha>`).
2. Run `git status` and diff the working tree against the pre-rewrite state to confirm no
   unexpected files were removed from disk.
3. Only after both checks pass, drop `refs/original/` and run `gc --aggressive`.

## Code-signing decision (v0.1.0)

**Decision: no certificate.** Corvus Capture v0.1.0 ships unsigned.

**Rationale:**

- An EV (Extended Validation) code-signing certificate costs roughly $300-500+/yr and
  requires business identity vetting (Dun & Bradstreet lookup, notarized documents, etc.).
  That cost and process is not justified for a free, solo-maintained utility.
- A cheaper OV (Organization Validation) certificate does **not** solve the problem it might
  appear to solve: since a Microsoft policy change circa 2020, only EV certificates grant
  instant SmartScreen application reputation. An OV certificate would remove the "unknown
  publisher" ambiguity in the file's properties, but Windows SmartScreen would still show
  the same "Windows protected your PC" reputation-based warning until enough
  downloads/time accrue reputation for that binary. Paying for an OV cert would therefore
  add cost without removing the SmartScreen prompt users actually see.
  *(This EV-vs-OV distinction was researched during v0.1.0 planning and should be
  re-verified against current Microsoft policy if code-signing is reconsidered for a
  future release — Microsoft has changed SmartScreen reputation rules before and may do
  so again. The original research notes were part of local, gitignored planning history
  and are not part of this repository.)*

**Executed alternative (compensating controls):**

1. Publish a SHA-256 checksum of the exact released binary as an immutable GitHub release
   asset (`SHA256SUMS.txt`), not as editable release-notes text, so it cannot be silently
   altered after publication.
2. Submit the binary to Microsoft for both AV false-positive analysis and SmartScreen
   application-reputation analysis (see the next section).
3. Document the SmartScreen "More info -> Run anyway" bypass for early adopters in the
   release notes and README, conditioned on checksum verification first.

**Decision superseded (2026-09-03, same day):** After initial publication, the maintainer's
existing **Azure Trusted Signing** account (already used for AgrusScanner) was applied to this
project — a pay-per-use signing service with none of the EV-certificate cost/vetting burden
the analysis above rejected. `CorvusCapture.exe` was Authenticode-signed
(signer `CN=Joseph Fago, O=Joseph Fago, L=Newark, S=New Jersey, C=US`, timestamped) and the
v0.1.0 release assets were replaced in place with the signed binary and its new checksum.
The compensating controls below (checksums, Microsoft submissions, MOTW test) remain valid
but are now defense-in-depth rather than the primary mitigation; SmartScreen reputation for
Trusted Signing-signed binaries accrues substantially faster than for unsigned ones.

**v0.1.0 release details:**

- Date: 2026-09-03 (assets replaced same day with the Trusted Signing-signed binary)
- SHA-256 (signed exe): `FA242C711F4BCDEE802966F371AF2675AF3C8745F0772C655E61CBBF24365F63`
- Signature: Azure Trusted Signing, `CN=Joseph Fago`, verify via `Get-AuthenticodeSignature`
- Release: https://github.com/NYBaywatch/CorvusCapture/releases/tag/v0.1.0

## Post-publish Microsoft submissions (manual)

These are two separate Microsoft systems with separate review timelines. Both must be
submitted; submitting only one leaves the other class of warning unresolved even if the
other clears.

1. **Portal 1 — AV false-positive submission (WDSI file submission).**
   URL: https://www.microsoft.com/en-us/wdsi/filesubmission
   Submit `CorvusCapture.exe` as a suspected false positive. Include the public release URL
   (https://github.com/NYBaywatch/CorvusCapture/releases/tag/v0.1.0) and the SHA-256 hash
   above as supporting detail. This addresses Microsoft Defender antivirus detections.

2. **Portal 2 — SmartScreen application-reputation submission.**
   URL: https://www.microsoft.com/en-us/wdsi/AppRepSubmission
   Submit the same binary for the "unrecognized publisher" SmartScreen prompt
   ("Windows protected your PC"). This is a distinct system from Portal 1 — reputation
   here accrues from both explicit review and aggregate download volume/age.

**Expected timeline:** The AV false-positive verdict from Portal 1 typically returns within
days. SmartScreen reputation from Portal 2 accrues over a longer period tied to download
volume and does not clear instantly, even after a clean AV verdict. **A persisting
SmartScreen prompt after a clean AV verdict is the expected outcome for a new release, not
a release blocker** — it is documented for users in the release notes and README rather
than treated as something to fix before announcing.

## Fresh-VM Mark-of-the-Web verification (required before announcement)

This test confirms a stranger's actual download-and-run experience matches what the
release notes promise. It must be performed on a clean Windows 11 VM with Microsoft
Defender enabled and current virus definitions — not on a development machine, and not by
copying the file over a network share or local copy (that does not apply a Mark-of-the-Web
zone identifier and will not reproduce the SmartScreen condition a real downloader hits).

**Procedure:**

1. On the clean VM, open a browser and download `CorvusCapture.exe` directly from the
   public release page (https://github.com/NYBaywatch/CorvusCapture/releases/tag/v0.1.0),
   not via a network share or removable media copy.
2. Confirm the file carries the Mark-of-the-Web zone identifier:
   ```
   Get-Content -Path CorvusCapture.exe -Stream Zone.Identifier
   ```
   This must return zone data (e.g. `ZoneId=3`) confirming Windows marked the file as
   downloaded from the internet.
3. Verify the checksum against the published hash:
   ```
   certutil -hashfile CorvusCapture.exe SHA256
   ```
   Compare the output to `FA242C711F4BCDEE802966F371AF2675AF3C8745F0772C655E61CBBF24365F63`
   (the signed binary), and confirm the digital signature is valid:
   ```
   Get-AuthenticodeSignature CorvusCapture.exe
   ```
4. Run the exe. Record whether Microsoft Defender flags it, and exactly what SmartScreen
   shows (if anything).
5. If SmartScreen appears, click `More info`, then `Run anyway`, and confirm the tray icon
   appears and pressing `F9` produces a saved file.
6. Record the observed outcome and the date below.

**Announcement gate:** v0.1.0 may only be announced publicly after this section records a
**pass** (Defender clean; SmartScreen prompt, if any, bypassable via `More info` -> `Run
anyway` and consistent with the README's documented workaround).

### Result

- Date: _(not yet performed)_
- Defender verdict: _(pending)_
- SmartScreen outcome: _(pending)_
- Tray icon / F9 capture confirmed: _(pending)_
- **Pass/Fail:** _(pending — announcement is blocked until this is filled in with a pass)_
