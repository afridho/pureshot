#!/usr/bin/env python3
"""Install the GNOME 50 companion locally; explicitly authorize native binaries."""
import argparse
import datetime
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile


def private_dir(path):
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
        raise SystemExit(f"Unsafe directory: {path}")
    path.chmod(0o700)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--allow-executable', action='append', default=[], metavar='PATH',
                        help='Add an existing, trusted PureShot native binary (repeatable).')
    parser.add_argument('--no-enable', action='store_true', help='Install without attempting enable.')
    args = parser.parse_args()
    binaries = []
    for value in args.allow_executable:
        path = Path(value).expanduser().resolve(strict=True)
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f'Not an executable file: {path}')
        # /proc/PID/exe identifies the interpreter for scripts, not the script.
        with path.open('rb') as binary:
            if binary.read(4) != b'\x7fELF':
                parser.error(f'Expected a native ELF executable, not a script: {path}')
        binaries.append(str(path))

    config = Path(os.environ.get('XDG_CONFIG_HOME', Path.home() / '.config')) / 'pureshot-extension'
    private_dir(config)
    allowlist = config / 'allowed-executables.json'
    paths = []
    if allowlist.exists() or allowlist.is_symlink():
        info = allowlist.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise SystemExit(f'Unsafe allowlist: {allowlist}')
        paths = json.loads(allowlist.read_text())
        if not isinstance(paths, list) or any(not isinstance(p, str) or not Path(p).is_absolute() for p in paths):
            raise SystemExit('Existing allowlist must be an array of absolute paths')
    paths = sorted(set(paths + binaries))
    fd, temporary = tempfile.mkstemp(prefix='.allowlist-', dir=config)
    try:
        with os.fdopen(fd, 'w') as output:
            json.dump(paths, output, indent=2)
            output.write('\n')
        os.replace(temporary, allowlist)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)

    extensions = Path(os.environ.get('XDG_DATA_HOME', Path.home() / '.local/share')) / 'gnome-shell/extensions'
    extensions.mkdir(parents=True, exist_ok=True)
    destination = extensions / 'pureshot@afridho'
    if destination.exists() or destination.is_symlink():
        stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S-%f')
        backup = extensions.parent / 'pureshot-extension-backups' / stamp / destination.name
        backup.parent.mkdir(parents=True, mode=0o700)
        destination.rename(backup)
        print(f'Previous extension backed up to {backup}')
    destination.mkdir(mode=0o700)
    for filename in ('metadata.json', 'extension.js', 'window-corners.js'):
        shutil.copyfile(Path(__file__).resolve().parent / filename, destination / filename)
    schema_src = Path(__file__).resolve().parent / 'schemas'
    schema_dst = destination / 'schemas'
    shutil.copytree(schema_src, schema_dst)
    result = subprocess.run(['glib-compile-schemas', str(schema_dst)], check=False)
    if result.returncode:
        raise SystemExit('glib-compile-schemas failed; the area-capture shortcut will not work')
    print(f'Installed {destination}')
    print(f'Allowed executables ({allowlist}): {paths}')
    if not args.no_enable:
        result = subprocess.run(['gnome-extensions', 'enable', 'pureshot@afridho'], check=False)
        if result.returncode:
            print('Shell has not discovered the extension yet. Log out/in, then run:')
            print('  gnome-extensions enable pureshot@afridho')
        else:
            print('Enable requested. Check: gnome-extensions info pureshot@afridho')
    print('Installed code updates require a logout/login to reload on Wayland. No Shell restart or unsafe mode change performed.')


if __name__ == '__main__':
    main()
