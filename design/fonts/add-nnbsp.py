# Adds a narrow no-break space (U+202F) to an instanced Recursive, which ships without one, so
# digit groups look the same in every browser instead of each drawing another font's space.
# One step of the subset build in docs/DESIGN.md ("Type"). Needs fontTools.
#
#   python3 add-nnbsp.py rec.ttf
import sys

from fontTools.ttLib import TTFont
from fontTools.ttLib.tables._g_l_y_f import Glyph

NAME = "uni202F"
# Half the word space (300): what Chromium drew from its fallback before, about 0.15 em.
WIDTH = 150

path = sys.argv[1]
font = TTFont(path)
if NAME in font.getGlyphOrder():
    sys.exit(f"{path} already has {NAME}")
# Read every table before the glyph count changes under them.
for tag in font.keys():
    font[tag]

order = [*font.getGlyphOrder(), NAME]
font.setGlyphOrder(order)
font["glyf"].glyphOrder = order
font["glyf"][NAME] = Glyph()
font["hmtx"][NAME] = (WIDTH, 0)
font["gvar"].variations[NAME] = []

# The same width at every weight and in monospace: map it to an advance that never varies, the
# digit zero's (600 everywhere, checked below).
hvar = font["HVAR"].table
hvar.AdvWidthMap.mapping[NAME] = hvar.AdvWidthMap.mapping["zero"]

for table in font["cmap"].tables:
    if table.isUnicode():
        table.cmap[0x202F] = NAME

font.save(path)

# Check: the new space keeps its width at the corners of both axes.
from fontTools.varLib import instancer  # noqa: E402

for location in ({"wght": 300, "MONO": 0}, {"wght": 800, "MONO": 1}):
    static = instancer.instantiateVariableFont(TTFont(path), location)
    assert static["hmtx"][NAME][0] == WIDTH, (location, static["hmtx"][NAME])
    assert static["hmtx"]["zero"][0] == 600, (location, static["hmtx"]["zero"])
