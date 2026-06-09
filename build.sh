#!/bin/bash
cd frontend && trunk build --release && cd ..
cargo build -p bettertest --release