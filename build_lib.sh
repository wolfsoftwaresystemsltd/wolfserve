#!/bin/bash
cd wolflib && cargo build --release
echo "Library built at $(pwd)/target/release/libwolflib.so"
