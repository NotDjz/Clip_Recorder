# Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org)

## Team roles

- Committers and reviewers: [NotDjz](https://github.com/NotDjz)
- Approvers: [NotDjz](https://github.com/NotDjz)

Every change reaches `master` through a pull request that must pass the required checks (tests, end-to-end tests,
mutation tests, dependency audit, secret scanning). Every release is built from this repository's source by GitHub
Actions (`.github/workflows/release.yml`), with SLSA build provenance, and is published, and signed, only after the
approver approves it.

## What is signed

`clip-recorder-<version>-x64.msi` and the `clip-recorder.exe` it installs, both built from this repository. The MSI
also carries `ffmpeg.exe`, an unmodified, hash-pinned build from [gyan.dev](https://www.gyan.dev/ffmpeg/builds/)
(GPL v3), which is not signed by this project.

## Privacy policy

This program will not transfer any information to other networked systems unless specifically requested by the user
or the person installing or operating it.

Clip Recorder records the screen and sound only into a local buffer, and writes a clip only when the user presses its
hotkey, into a folder the user chooses. FFmpeg, which it runs locally to encode, makes no network connection either.

## System changes

The installer puts Clip Recorder in Program Files and makes it start with Windows for the user who installs it; the
installer says so, and Settings > "Start with Windows" switches it off. Uninstall it from Windows Settings > Apps:
this removes the program and the start-up entry, and keeps your clips and settings.
