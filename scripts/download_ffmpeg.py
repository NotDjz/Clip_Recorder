"""Telecharge ffmpeg.exe depuis gyan.dev pour le bundle PyInstaller."""

import io
import os
import zipfile
import urllib.request

URL = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"
# ffmpeg.exe lives at the REPO ROOT (this script sits in scripts/): that is where
# build.bat's --add-data expects it, and where the app looks for it when run
# from source.
DEST = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NEEDED = {"ffmpeg.exe"}


def main():
    if all(os.path.exists(os.path.join(DEST, n)) for n in NEEDED):
        print("ffmpeg.exe deja present, skip.")
        return

    print("Telechargement de FFmpeg (~80 MB)...")
    data = urllib.request.urlopen(URL).read()
    print("Extraction de ffmpeg.exe...")

    with zipfile.ZipFile(io.BytesIO(data)) as zf:
        for entry in zf.namelist():
            basename = os.path.basename(entry)
            if basename in NEEDED:
                print(f"  -> {basename}")
                with zf.open(entry) as src, open(os.path.join(DEST, basename), "wb") as dst:
                    dst.write(src.read())

    missing = [n for n in NEEDED if not os.path.exists(os.path.join(DEST, n))]
    if missing:
        print(f"ERREUR: fichiers manquants: {missing}")
        raise SystemExit(1)

    print("OK!")


if __name__ == "__main__":
    main()
