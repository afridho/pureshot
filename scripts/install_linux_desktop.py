#!/usr/bin/env python3
"""Install per-user GNOME desktop identity and icons; never requires sudo."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--executable", type=Path, default=Path.home() / ".local/bin/pureshot")
args = parser.parse_args()
executable = args.executable.expanduser().resolve(strict=True)
if not executable.is_file() or not os.access(executable, os.X_OK):
    parser.error("--executable must be an executable file")
if any(c in str(executable) for c in '\n\r'):
    parser.error("executable path cannot contain line breaks")
root = Path(__file__).resolve().parents[1]
data = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share"))
applications = data / "applications"
applications.mkdir(parents=True, exist_ok=True)
# Desktop Exec quoting is not shell quoting. Escape reserved characters and %.
quoted = str(executable).replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%')
for name in ("com.pureshot.app", "pureshot"):
    icon = data / "icons/hicolor/128x128/apps" / f"{name}.png"
    icon.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(root / "src-tauri/icons/linux-dock.png", icon)
launcher = applications / "com.pureshot.app.desktop"
launcher.write_text(f'''[Desktop Entry]
Type=Application
Name=PureShot
Comment=Capture screenshots and extract text locally
Exec="{quoted}"
Icon=com.pureshot.app
Terminal=false
Categories=Utility;Graphics;
StartupWMClass=com.pureshot.app
''')
for command in (["update-desktop-database", str(applications)],
                ["gtk-update-icon-cache", "--force", "--ignore-theme-index", str(data / "icons/hicolor")]):
    if shutil.which(command[0]):
        subprocess.run(command, check=False)
print(f"Installed {launcher}")
print("Restart PureShot to use the GTK app ID. If the old icon is pinned, unpin it and pin the restarted window.")
