"""Fetch the pinned ffmpeg.exe that the PyInstaller bundle ships."""

import hashlib
import io
import os
import urllib.request
import zipfile

# Pinned, never "latest": the exact build every release has shipped. The hash
# is of ffmpeg.exe itself, so a swapped or tampered download can never reach
# the installer. Bump the URL and the hash together.
URL = "https://github.com/GyanD/codexffmpeg/releases/download/8.1/ffmpeg-8.1-full_build.zip"
FFMPEG_SHA256 = "d1e2a156261ecc675081943197a85f08f2868784a0af499171ede89353edad31"

# ffmpeg.exe lives at the REPO ROOT (this script sits in scripts/): that is where
# build.bat's --add-data expects it, and where the app looks for it when run
# from source.
EXE = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                   "ffmpeg.exe")


def main():
    if os.path.exists(EXE):
        with open(EXE, "rb") as f:          # not file_digest: that is 3.11+
            if hashlib.sha256(f.read()).hexdigest() == FFMPEG_SHA256:
                print("ffmpeg.exe already present and verified, skipping.")
                return
        # Removed before downloading, so a failed or refused download cannot
        # leave this unverified binary where the next build would bundle it.
        print("ffmpeg.exe present but not the pinned build, removing it.")
        try:
            os.remove(EXE)
        except OSError as e:
            print(f"ERROR: cannot remove the unverified ffmpeg.exe ({e}). "
                  "Close whatever is using it and run again.")
            raise SystemExit(1)

    print(f"Downloading {URL} (~250 MB)...")
    data = urllib.request.urlopen(URL, timeout=120).read()
    with zipfile.ZipFile(io.BytesIO(data)) as zf:
        member = next((n for n in zf.namelist()
                       if os.path.basename(n) == "ffmpeg.exe"), None)
        if member is None:
            print("ERROR: no ffmpeg.exe in the downloaded archive.")
            raise SystemExit(1)
        exe = zf.read(member)

    # Checked in memory, before anything is written: an unverified binary must
    # not even land on disk where a later build could pick it up.
    if hashlib.sha256(exe).hexdigest() != FFMPEG_SHA256:
        print("ERROR: the downloaded ffmpeg.exe does not match the pinned "
              "SHA-256. Refusing to use it.")
        raise SystemExit(1)

    # Written aside, then swapped in: an interrupted write never leaves a
    # truncated ffmpeg.exe under the real name.
    part = EXE + ".part"
    with open(part, "wb") as f:
        f.write(exe)
    os.replace(part, EXE)
    print("OK: ffmpeg.exe verified and written.")


if __name__ == "__main__":
    main()
