#!/bin/sh
cargo clippy --fix --allow-dirty --allow-staged
cargo fmt

# hacky slop to enforce one blank line between functions
gawk -i inplace 'BEGINFILE { prev = "" } prev == "}" && $0 != "" { print "" } { print; prev = $0 }' */src/*.rs

ruff check --fix pylib
ruff format pylib

uvx ty check --error-on-warning pylib
