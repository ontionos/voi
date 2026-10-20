# How the disk image's window looks when it opens. Read by dmgbuild.
#
# Finder keeps this in a .DS_Store next to the files: window size, icon
# positions, background picture. Written here rather than by opening the
# image and dragging things about, so it comes out the same every time.
#
# The numbers live in make_dmg_bg.py, next door, because the arrow is drawn
# against them — a background whose arrow points somewhere the icons are not
# is the one mistake worth designing out.

import importlib.util
import os
import pathlib

_spec = importlib.util.spec_from_file_location(
    "voi_dmg_bg", pathlib.Path(defines["here"]) / "make_dmg_bg.py"
)
bg = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bg)

app = defines["app"]
appname = os.path.basename(app)

# What goes in it: the app, and somewhere to drop it.
files = [app]
symlinks = {"Applications": "/Applications"}
icon_locations = {
    appname: (bg.APP_X, bg.ICON_Y),
    "Applications": (bg.DEST_X, bg.ICON_Y),
}

# What it looks like. Small: there are two things in it and one instruction.
background = defines["background"]
# The frame, not the content: Finder counts the title bar in these bounds, and
# for anyone with View → Show Tab Bar on, the tab bar as well. Without room for
# both, the bottom of the picture is cut off for them. Without a tab bar the
# window is just that much taller, and the picture carries on below.
TITLE_BAR = 28
TAB_BAR = 36
window_rect = ((260, 240), (bg.WIDTH, bg.HEIGHT + TITLE_BAR + TAB_BAR))
default_view = "icon-view"
icon_size = 100
text_size = 12
label_pos = "bottom"
arrange_by = None
grid_offset = (0, 0)
grid_spacing = 100
scroll_position = (0.0, 0.0)

# None of the chrome. This is not a folder anybody navigates.
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False

# The mounted volume carries the app's own icon rather than a blank disk.
# `icon`, not `badge_icon`: badging paints the icon onto a picture of a disk
# and needs PyObjC to do it, which is a dependency for a detail nobody looks
# at twice.
icon = defines["icon"]

# Compressed and read-only, which is what a distributed image should be.
format = "UDZO"
