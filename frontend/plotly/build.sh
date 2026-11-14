#!/bin/sh
# Builds ../assets/plotly.vN.min.js, which is checked in: building the frontend needs no npm. A
# changed bundle needs a new N, since assets are cached forever.
set -e
cd "$(dirname "$0")"
npm install --no-save
npx esbuild entry.js --bundle --minify --legal-comments=none --loader:.css=empty \
    --define:global=window --outfile=../assets/plotly.v1.min.js
cp node_modules/plotly.js/LICENSE ../assets/plotly-LICENSE.txt
