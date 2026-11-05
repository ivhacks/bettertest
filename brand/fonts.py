"""Static instances of the UI's variable fonts, for Typst, which only draws a variable font's default
instance. One family per width on Archivo's axis the guide uses ("Archivo 62" to "Archivo 100"), each at the
weights it uses. Written to brand/fonts/ (not in git): run before compiling the guide."""

from pathlib import Path

from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

ASSETS = Path(__file__).parent.parent / "frontend" / "assets"
OUT = Path(__file__).parent / "fonts"
WEIGHTS = {400: "Regular", 500: "Medium", 600: "SemiBold", 700: "Bold"}


def instance(source: str, family: str, width: float, weight: int) -> None:
    variable = TTFont(ASSETS / source)
    # The UI's Martian Mono is already pinned to one width.
    axes = {a.axisTag for a in variable["fvar"].axes}
    limits = {
        tag: value for tag, value in (("wdth", width), ("wght", weight)) if tag in axes
    }
    font = instancer.instantiateVariableFont(variable, limits)
    font.flavor = None
    style = WEIGHTS[weight]
    names = font["name"]
    for record in list(names.names):
        if record.nameID in (16, 17, 25):
            names.removeNames(nameID=record.nameID)
    names.setName(family, 1, 3, 1, 0x409)
    names.setName(style, 2, 3, 1, 0x409)
    names.setName(f"{family} {style}", 4, 3, 1, 0x409)
    names.setName(f"{family.replace(' ', '')}-{style}", 6, 3, 1, 0x409)
    font["OS/2"].usWeightClass = weight
    font.save(OUT / f"{family.replace(' ', '')}-{style}.ttf")


OUT.mkdir(exist_ok=True)
for width in (62, 70, 80, 90, 100):
    for weight in WEIGHTS:
        instance("archivo.v1.woff2", f"Archivo {width}", width, weight)
for weight in (400, 700):
    instance("martian-mono.v1.woff2", "Martian Mono 75", 75, weight)
