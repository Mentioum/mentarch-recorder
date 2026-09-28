#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
cargo build --release --locked
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
bin_dir="$HOME/.local/bin"
app_id="io.github.mentioum.MentarchRecorder"

install -Dm755 target/release/mentarch-recorder "$bin_dir/mentarch-recorder"
install -Dm644 "data/$app_id.svg" "$data_dir/icons/hicolor/scalable/apps/$app_id.svg"
install -Dm644 data/waybar-menu.ui "$data_dir/mentarch-recorder/waybar-menu.ui"
# Use an absolute, desktop-entry-quoted executable so launcher PATH settings do
# not determine whether the application can start.
python3 - "$bin_dir/mentarch-recorder" "data/$app_id.desktop" "$data_dir/applications/$app_id.desktop" <<'PY'
import pathlib
import sys
executable, source, destination = sys.argv[1:]
escaped = executable.replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%')
# Desktop entries unescape values before parsing Exec quoting.
escaped = escaped.replace('\\', '\\\\')
entry = pathlib.Path(source).read_text().replace('Exec=mentarch-recorder', f'Exec="{escaped}"')
path = pathlib.Path(destination)
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(entry)
PY
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data_dir/applications"
fi
printf 'Installed Window Recorder to %s\n' "$bin_dir/mentarch-recorder"
