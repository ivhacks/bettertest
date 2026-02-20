#!/bin/bash
cd "$(dirname "$0")/frontend" && trunk build && cd .. && cargo build -p bettertest
