#!/bin/bash
# Builds style_guide.pdf from style_guide.typ. Typst draws only a variable font's default instance, so first
# make static instances of the UI's own fonts (needs python3 with fonttools and brotli).
set -e
cd "$(dirname "$0")"
python3 fonts.py
typst compile --font-path fonts --ignore-system-fonts style_guide.typ style_guide.pdf
