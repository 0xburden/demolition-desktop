#!/bin/sh
set -eu

binary=${1:?usage: bundle.sh PATH_TO_DEMOLITION OUTPUT_DIRECTORY}
out_dir=${2:?usage: bundle.sh PATH_TO_DEMOLITION OUTPUT_DIRECTORY}
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
app="$out_dir/Demolition.app"

mkdir -p "$app/Contents/MacOS"
install -m 755 "$binary" "$app/Contents/MacOS/demolition"
install -m 644 "$script_dir/Info.plist" "$app/Contents/Info.plist"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
