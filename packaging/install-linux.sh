#!/usr/bin/env bash
# Install an extracted portable package for this user.
set -euo pipefail
source_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
destination="${XDG_DATA_HOME:-$HOME/.local/share}/vole"
command_directory="$HOME/.local/bin"
application_directory="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
if command -v pgrep >/dev/null && pgrep -u "$(id -u)" -x vole >/dev/null; then
  echo 'Close Vole before replacing an installed copy.' >&2
  exit 1
fi
if [[ -e "$command_directory/vole" && ! -L "$command_directory/vole" ]]; then
  echo "A file already exists at $command_directory/vole. Move it before installing Vole." >&2
  exit 1
fi
mkdir -p "$destination" "$command_directory" "$application_directory"
if [[ "$source_directory" != "$destination" ]]; then
  cp -a "$source_directory/." "$destination/"
fi
ln -sfn "$destination/vole" "$command_directory/vole"
# Desktop launchers require an absolute, quoted executable path.
escaped_destination=${destination//\\/\\\\}
escaped_destination=${escaped_destination//\"/\\\"}
escaped_destination=${escaped_destination//\$/\\\$}
escaped_destination=${escaped_destination//\`/\\\`}
{
  printf '[Desktop Entry]\nType=Application\nName=Vole\n'
  printf 'Comment=Assemble and simulate teaching machines\n'
  printf 'Exec="%s/vole"\n' "$escaped_destination"
  printf 'Icon=%s/vole.png\n' "$destination"
  printf 'Terminal=false\nCategories=Development;Education;\nStartupWMClass=dev.vole.Workbench\n'
} > "$application_directory/dev.vole.Workbench.desktop"
printf 'Installed Vole in %s. Open Vole from your application launcher.\n' "$destination"
printf 'For terminal launch, include %s in PATH.\n' "$command_directory"
