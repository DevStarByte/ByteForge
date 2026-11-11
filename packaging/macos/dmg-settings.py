# dmgbuild settings for the ByteForge .dmg: the window looks like a ByteForge editor (dmg-background.svg).
# bundle.sh runs: dmgbuild -s dmg-settings.py -D app=<ByteForge.app> -D background=<bg.tiff> -D icon=<icns> ByteForge <out.dmg>
import os.path

app = defines["app"]  # noqa: F821 (provided by dmgbuild)
app_name = os.path.basename(app)

format = "UDZO"
filesystem = "HFS+"
files = [app]
symlinks = {"Applications": "/Applications"}
icon = defines["icon"]  # noqa: F821  volume icon

background = defines["background"]  # noqa: F821  640x400, with a 1280x800 Retina image in the same TIFF
window_rect = ((200, 160), (640, 400))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False

# Icon centers; the labels land on the highlighted "current line" of the background.
icon_size = 96
text_size = 13
icon_locations = {app_name: (170, 196), "Applications": (470, 196)}
arrange_by = None
