<!-- SPDX-License-Identifier: Apache-2.0 -->
# Releasing photodedup

The owner pushes commits and version tags. The GitHub MCP connection remains
read-only. All commits and tags use the repository-local identity and SSH
signing configuration. Release builds have no build cache.

## First-release status (2026-11-06)

- CI at `56e0713` passed all seven jobs, including Windows engine tests.
- The models workflow at `66d10eb` rebuilt the fp32/fp16 artifacts with the
  manifest hashes. Model runtime integration is deferred past 0.1.0.
- Release dry run #3 (`41015544543`) built all four platform legs successfully.
  It predates the sidecar rename and Linux deb/rpm packages.
- Dry run #5 (`41028261327`) passed Linux after those changes. The Linux
  packages were subsequently tested in isolated containers.
- Public dry run #6 (`42241545356`, commit `0b163fd`) passed the Apple Silicon
  build, sidecar checks, public attestation and artifact uploads. The owner
  installed and launched its DMG successfully; macOS required the documented
  Privacy & Security workaround. Apply/Undo and other smoke-test steps were
  not explicitly reported.
- The owner chose **Apple Silicon only for 0.1.0**. Intel Mac, Windows and
  Linux installer validation is deferred to later releases.
- The repository is public and has no tags or releases (verified through the
  GitHub API on November 6). The workflow enables
  provenance attestations automatically when `repository.private` is false;
  that step passed in the Apple Silicon dry run.
- The tag-only publish job has never run. A manual dry run does not exercise it.

`FIRST-CI-RUN.md` is the historical record of initial checks and failures;
its statements about workflows never having run are historical.

## Build and inspect the candidate

1. Push the prepared release commit to `main` and confirm CI is green.
2. For another dry run, choose **release → Run workflow**, branch **main**,
   **legs: macos-arm**. This builds the Apple Silicon DMG, computes checksums,
   creates a public-repository attestation, and uploads artifacts. It does not
   publish a GitHub release.
3. Inspect the build, attestation, artifact uploads and checksum file.
   Keep the no-cache and minimum-permissions
   design intact if a fix is needed.
4. Smoke-test the Apple Silicon installer with a synthetic library and
   disposable database: launch; confirm bundled ffmpeg/ffprobe are found;
   scan photos and video in Default mode; review and quarantine a file;
   Apply; Undo; quit and reopen to confirm the review resumes. HEIC coverage
   requires permitted HEIC samples. Record any untested platform explicitly.
5. Compare installer SHA-256 values with the matching `SHA256SUMS-*.txt`.
   Verify provenance with:

   ```sh
   gh attestation verify <installer> --repo amatemeow/photodedup
   ```

## Tag and publish

After the candidate passes, the owner creates and pushes only its version tag
at the reviewed commit (replace the placeholder with the full candidate SHA):

```sh
git tag -s v0.1.0 <reviewed-candidate-sha> -m 'photodedup 0.1.0'
git tag -v v0.1.0
git push origin refs/tags/v0.1.0
```

The tag triggers another Apple Silicon build. Only after it succeeds does
the isolated `publish` job download that run's installer and checksum file
and attach them to the GitHub release. It publishes automatically; the current
workflow does not create a draft or prerelease. Do not push a version tag just
to test a build. Do not move a published tag to repair a release.

Inspect the first publish run and confirm that the release contains
`photodedup_0.1.0_aarch64.dmg` and `SHA256SUMS-macos-15.txt`.
Verify a downloaded release asset again. The publish job reads the release
description from `docs/releases/<tag>.md`; prepare a matching notes file before
each future tag. Other platforms remain selectable for manual dry runs; expand
the tag build matrix deliberately when their installers are ready to ship.

## Public repository follow-up from the original handoff

Enable code scanning, secret scanning and push protection in repository
settings, and require approval for outside collaborators' workflow runs.
Keep the GitHub MCP read-only and in lockdown. These settings are owner
actions and have not been verified by this checklist.
