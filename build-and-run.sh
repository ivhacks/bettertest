#!/bin/bash
cd "$(dirname "$0")"
cd frontend && trunk build && cd .. && cargo run -p bettertest
