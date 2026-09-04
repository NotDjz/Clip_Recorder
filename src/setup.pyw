"""
Clip Recorder — Setup.

Small installer: asks where to keep Clip Recorder, copies the bundled
app exe there, optionally creates a desktop shortcut, then launches it.
"""

import ctypes
import json
import os
import shutil
import subprocess
import sys
import tkinter as tk
from tkinter import filedialog

if getattr(sys, "frozen", False):
    BUNDLE_DIR = sys._MEIPASS
else:
    BUNDLE_DIR = os.path.dirname(os.path.abspath(__file__))

try:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
except Exception:
    try:
        ctypes.windll.user32.SetProcessDPIAware()
    except Exception:
        pass

BG = "#1e1e1e"
BG2 = "#2d2d2d"
BG3 = "#3c3c3c"
FG = "#e0e0e0"
FG2 = "#999999"
ACCENT = "#ff4444"
FONT = ("Segoe UI", 10)
FONT_B = ("Segoe UI", 10, "bold")
FONT_S = ("Segoe UI", 9)

DEFAULTS = {
    "monitor": 0,
    "fps": 60,
    "buffer_seconds": 30,
    "output_folder": "",
    "loopback_device": "",
    "mic_device": "",
    "hotkey": "Ctrl+Alt+R",
}


# {B97D20BB-F46A-4C97-BA10-5E3608430854} — the per-user Startup folder. Resolved
# through the API rather than a hand-built %APPDATA% path so a relocated folder
# still works. clip_recorder.pyw carries its own copy of this: the two scripts are
# standalone by design (Setup never imports the app), so it is structural, not an
# oversight — change one and change the other.
class _GUID(ctypes.Structure):
    _fields_ = [("Data1", ctypes.c_ulong), ("Data2", ctypes.c_ushort),
                ("Data3", ctypes.c_ushort), ("Data4", ctypes.c_ubyte * 8)]


# {B4BFCC3A-DB2C-424C-B029-7FE99A87C641} — with OneDrive Known Folder Backup the
# desktop lives under ~/OneDrive/Desktop, so a hand-built ~/Desktop silently misses.
FOLDERID_DESKTOP = _GUID(
    0xB4BFCC3A, 0xDB2C, 0x424C,
    (ctypes.c_ubyte * 8)(0xB0, 0x29, 0x7F, 0xE9, 0x9A, 0x87, 0xC6, 0x41),
)


FOLDERID_STARTUP = _GUID(
    0xB97D20BB, 0xF46A, 0x4C97,
    (ctypes.c_ubyte * 8)(0xBA, 0x10, 0x5E, 0x36, 0x08, 0x43, 0x08, 0x54),
)


def _known_folder(folderid, fallback):
    try:
        path_ptr = ctypes.c_wchar_p()
        hr = ctypes.windll.shell32.SHGetKnownFolderPath(
            ctypes.byref(folderid), 0, None, ctypes.byref(path_ptr)
        )
        if hr == 0 and path_ptr.value:
            path = path_ptr.value
            ctypes.windll.ole32.CoTaskMemFree(path_ptr)
            return path
    except Exception:
        pass
    return fallback


def _get_startup_folder():
    return _known_folder(FOLDERID_STARTUP, os.path.join(
        os.environ.get("APPDATA", ""), "Microsoft", "Windows",
        "Start Menu", "Programs", "Startup"))


def _get_desktop_folder():
    return _known_folder(FOLDERID_DESKTOP,
                         os.path.join(os.path.expanduser("~"), "Desktop"))


def _create_shortcut(target_exe, lnk_path):
    """One helper for both shortcuts: the script is identical, only where the
    .lnk lands differs."""
    try:
        # Paths travel in the ENVIRONMENT, never interpolated into the script.
        # Escaping cannot fix this: PowerShell takes U+2018/2019/201A/201B as
        # string delimiters, and the console codepage rewrites them to a plain
        # apostrophe in transit — a quote can appear AFTER the escape ran. The
        # install directory comes from the Browse dialog, so it is user-chosen.
        ps_script = (
            "$s = New-Object -ComObject WScript.Shell;"
            "$sc = $s.CreateShortcut($env:CR_LNK);"
            "$sc.TargetPath = $env:CR_TARGET;"
            "$sc.WorkingDirectory = Split-Path -Parent $env:CR_TARGET;"
            "$sc.IconLocation = $env:CR_TARGET;"
            "$sc.Save()"
        )
        subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", ps_script],
            capture_output=True, timeout=10, creationflags=0x08000000,
            env={**os.environ, "CR_LNK": lnk_path, "CR_TARGET": target_exe},
        )
    except Exception:
        pass


def _app_is_running():
    h = ctypes.windll.kernel32.CreateMutexW(None, False, "ClipRecorder_SingleInstance")
    running = ctypes.windll.kernel32.GetLastError() == 183  # ERROR_ALREADY_EXISTS
    if h:
        ctypes.windll.kernel32.CloseHandle(h)
    return running


def _find_app_source():
    bundled = os.path.join(BUNDLE_DIR, "ClipRecorder.exe")
    if os.path.exists(bundled):
        return bundled
    # Dev convenience: running setup.pyw from source, look next to it in dist\
    dev_path = os.path.join(BUNDLE_DIR, "dist", "ClipRecorder.exe")
    if os.path.exists(dev_path):
        return dev_path
    return None


def main():
    import tkinter.messagebox

    app_source = _find_app_source()
    if not app_source:
        r = tk.Tk()
        r.withdraw()
        tkinter.messagebox.showerror(
            "Clip Recorder Setup",
            "Couldn't find ClipRecorder.exe to install.",
        )
        return

    result = {"path": "", "shortcut": True, "startup": True, "confirmed": False}

    dlg = tk.Tk()
    dlg.title("Clip Recorder — Setup")
    dlg.configure(bg=BG)
    dlg.resizable(False, False)
    dlg.attributes("-topmost", True)

    tk.Label(
        dlg, text="Where do you want to install Clip Recorder?", bg=BG, fg=FG, font=FONT_B,
    ).pack(padx=20, pady=(20, 5), anchor="w")
    tk.Label(
        dlg, text="The app and your settings will stay in this folder.",
        bg=BG, fg=FG2, font=FONT_S,
    ).pack(padx=20, pady=(0, 10), anchor="w")

    default_path = os.path.join(os.path.expandvars("%LOCALAPPDATA%"), "Programs", "ClipRecorder")
    path_var = tk.StringVar(value=default_path)
    row = tk.Frame(dlg, bg=BG)
    row.pack(padx=20, fill="x")
    entry = tk.Entry(row, textvariable=path_var, width=45, bg=BG3, fg=FG,
                      insertbackground=FG, relief="flat")
    entry.pack(side="left", fill="x", expand=True, ipady=4)

    def browse():
        folder = filedialog.askdirectory(title="Install folder", initialdir=path_var.get())
        if folder:
            path_var.set(folder)

    tk.Button(
        row, text="Browse...", command=browse,
        bg=BG3, fg=FG, activebackground=BG2, relief="flat",
    ).pack(side="left", padx=(6, 0))

    shortcut_var = tk.BooleanVar(value=True)
    tk.Checkbutton(
        dlg, text="Create a desktop shortcut", variable=shortcut_var,
        bg=BG, fg=FG, selectcolor=BG2, activebackground=BG, font=FONT_S,
    ).pack(padx=20, pady=(10, 0), anchor="w")

    startup_var = tk.BooleanVar(value=True)
    tk.Checkbutton(
        dlg, text="Start Clip Recorder when Windows starts", variable=startup_var,
        bg=BG, fg=FG, selectcolor=BG2, activebackground=BG, font=FONT_S,
    ).pack(padx=20, pady=(4, 0), anchor="w")

    def cancel():
        dlg.destroy()

    def confirm():
        result["path"] = path_var.get().strip() or default_path
        result["shortcut"] = shortcut_var.get()
        result["startup"] = startup_var.get()
        result["confirmed"] = True
        dlg.destroy()

    tk.Button(
        dlg, text="Install", command=confirm,
        bg=ACCENT, fg="#ffffff", font=FONT_B, relief="flat",
    ).pack(padx=20, pady=20, fill="x", ipady=6)

    dlg.protocol("WM_DELETE_WINDOW", cancel)
    dlg.update_idletasks()
    w, h = dlg.winfo_reqwidth(), dlg.winfo_reqheight()
    x = (dlg.winfo_screenwidth() - w) // 2
    y = (dlg.winfo_screenheight() - h) // 2
    dlg.geometry(f"+{x}+{y}")
    dlg.mainloop()

    if not result["confirmed"]:
        return

    if _app_is_running():
        r = tk.Tk()
        r.withdraw()
        tkinter.messagebox.showerror(
            "Clip Recorder Setup",
            "Clip Recorder is currently running.\n\n"
            "Please quit it (right-click the tray icon) and run Setup again.",
        )
        r.destroy()
        return

    target_dir = os.path.normpath(result["path"])
    try:
        os.makedirs(target_dir, exist_ok=True)
        new_exe = os.path.join(target_dir, "ClipRecorder.exe")
        shutil.copy2(app_source, new_exe)
        config_path = os.path.join(target_dir, "config.json")
        if not os.path.exists(config_path):
            with open(config_path, "w", encoding="utf-8") as f:
                json.dump(DEFAULTS, f, indent=2, ensure_ascii=False)
        if result["shortcut"]:
            _create_shortcut(new_exe, os.path.join(
                _get_desktop_folder(), "ClipRecorder.lnk"))
        # Reconcile, do not just create: on a re-install, unticking has to REMOVE
        # the existing shortcut, and re-ticking has to re-point it — otherwise a
        # changed install folder leaves Windows launching the old, deleted exe.
        startup_lnk = os.path.join(_get_startup_folder(), "ClipRecorder.lnk")
        if result["startup"]:
            _create_shortcut(new_exe, startup_lnk)
        elif os.path.exists(startup_lnk):
            try:
                os.remove(startup_lnk)
            except OSError:
                pass
        subprocess.Popen([new_exe])
    except Exception:
        r = tk.Tk()
        r.withdraw()
        tkinter.messagebox.showerror(
            "Clip Recorder Setup",
            f"Couldn't install to:\n{target_dir}\n\n"
            "Make sure Clip Recorder is closed and you have write access to this folder.",
        )
        r.destroy()


if __name__ == "__main__":
    main()
