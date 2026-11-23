#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds each extension's file, the same bytes from the same sources with
# the same Rust: the WebAssembly target, the workspace's release profile (one
# unit, no debug information, stripped), Cargo.lock as it is, and no path of
# the computer that built it (each remapped). Prints each file's SHA-256, for
# manifest.toml, and the Rust that made it: the hash holds for that version.
#
#     tools/build.sh              every extension
#     tools/build.sh skolengo     the ones named
#
# The files go in out/: <id>-<version>.wasm, and SHA256SUMS. Needs a Rust
# with the target `wasm32-unknown-unknown` (rustup target add
# wasm32-unknown-unknown). POSIX sh: it runs the same in the automation.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
all="skolengo planity"
names=${*:-$all}
target=wasm32-unknown-unknown
if [ ! -d "$(rustc --print target-libdir --target "$target" 2>/dev/null)" ]; then
    echo "build.sh: this Rust has no $target (rustup target add $target)." >&2
    exit 2
fi
out="$root/out"
mkdir -p "$out"
cargo_home=${CARGO_HOME:-$HOME/.cargo}
export CARGO_INCREMENTAL=0 SOURCE_DATE_EPOCH=0
export RUSTFLAGS="--remap-path-prefix=$root=/sioul-extensions --remap-path-prefix=$cargo_home=/cargo"
built=${CARGO_TARGET_DIR:-$root/target}
: > "$out/SHA256SUMS.new"
for id in $names; do
    version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$id/Cargo.toml" | head -n 1)
    cargo build --locked --release --target "$target" -p "$id"
    file="$id-$version.wasm"
    cp "$built/$target/release/$id.wasm" "$out/$file"
    (cd "$out" && sha256sum "$file") >> "$out/SHA256SUMS.new"
done
mv "$out/SHA256SUMS.new" "$out/SHA256SUMS"
echo "Built with $(rustc --version) ($(cargo --version)):"
cat "$out/SHA256SUMS"
