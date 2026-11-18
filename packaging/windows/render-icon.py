"""Renders packaging/windows/icon.svg into byteforge.ico (embedded into byteforge.exe by build.rs).

Needs Chrome or Edge for the SVG rendering and nothing else:  python packaging/windows/render-icon.py
"""
import os, shutil, struct, subprocess, sys, tempfile

SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
HERE = os.path.dirname(os.path.abspath(__file__))


def browser():
    for name in ("chrome", "msedge", "google-chrome", "chromium"):
        if path := shutil.which(name):
            return path
    for path in (r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                 r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                 r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"):
        if os.path.exists(path):
            return path
    sys.exit("Chrome or Edge is needed to render the icon")


def render(exe, svg, size, out_dir):
    html = os.path.join(out_dir, f"{size}.html")
    png = os.path.join(out_dir, f"{size}.png")
    with open(html, "w") as f:
        f.write(f'<html><body style="margin:0;background:transparent">'
                f'<img src="file:///{svg.replace(os.sep, "/")}" width="{size}" height="{size}" style="display:block">')
    subprocess.run([exe, "--headless", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=1",
                    "--default-background-color=00000000", f"--window-size={size},{size}",
                    f"--user-data-dir={os.path.join(out_dir, 'profile')}", f"--screenshot={png}", html],
                   check=True, capture_output=True)
    with open(png, "rb") as f:
        return f.read()


def main():
    exe = browser()
    svg = os.path.join(HERE, "icon.svg")
    with tempfile.TemporaryDirectory() as tmp:
        images = [(s, render(exe, svg, s, tmp)) for s in SIZES]
    # ICO with PNG-compressed entries (supported since Windows Vista)
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, data = b"", b""
    for size, png in images:
        dim = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset + len(data))
        data += png
    with open(os.path.join(HERE, "byteforge.ico"), "wb") as f:
        f.write(header + entries + data)
    print(f"wrote byteforge.ico ({', '.join(map(str, SIZES))} px)")


if __name__ == "__main__":
    main()
