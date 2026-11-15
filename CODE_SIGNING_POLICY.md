# Code signing policy

## Team roles

- Committers and reviewers: [NotDjz](https://github.com/NotDjz)
- Approvers: [NotDjz](https://github.com/NotDjz)

Every change reaches `master` through a pull request that must pass the required checks (tests, end-to-end tests,
mutation tests, dependency audit, secret scanning). Every release is built from this repository's source by GitHub
Actions (`.github/workflows/release.yml`), with SLSA build provenance, and is published only after the approver
approves it.

## Signing

Releases are not signed yet: SignPath Foundation, which signs open-source projects for free, will be asked again
once this one is better known. Until then:

- Windows SmartScreen warns when you run a new version's installer: "More info", then "Run anyway". Smart App
  Control, when it is on, blocks unsigned programs outright, with no way to run them.
- Instead, GitHub holds a build provenance attestation for each MSI. With the GitHub CLI signed in, this checks
  that the file was built by this repository's release workflow from that version's tag:

  ```
  gh attestation verify clip-recorder-<version>-x64.msi -R NotDjz/Clip_Recorder --source-ref refs/tags/v<version>
  ```

The MSI installs `clip-recorder.exe`, built from this repository, and `ffmpeg.exe`, an unmodified, hash-pinned build
from [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) (GPL v3).

## Privacy policy

This program will not transfer any information to other networked systems unless specifically requested by the user
or the person installing or operating it.

Clip Recorder records the screen and sound only into a local buffer, and writes a clip only when the user presses its
hotkey, into a folder the user chooses. FFmpeg, which it runs locally to encode, makes no network connection either.

## System changes

The installer puts Clip Recorder in Program Files and makes it start with Windows for the user who installs it; the
installer says so, and Settings > "Start with Windows" switches it off. Uninstall it from Windows Settings > Apps:
this removes the program and the start-up entry, and keeps your clips and settings.
