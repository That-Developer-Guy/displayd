#!/usr/bin/sh
RUSTFLAGS="-A warnings" cargo run --quiet -p displayctl -- --help 2>/dev/null