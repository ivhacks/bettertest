#!/bin/sh
cargo clippy --fix --allow-dirty --allow-staged
cargo fmt

ruff check --fix pylib
ruff format pylib

uvx ty check --error-on-warning pylib
