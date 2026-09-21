#!/usr/bin/env python3
"""Adds an applet id to the right wing of the COSMIC panel, backing up the config first."""
import shutil
import sys
from pathlib import Path

CONFIG = Path.home() / ".config/cosmic/com.system76.CosmicPanel.Panel/v1/plugins_wings"


def main(app_id: str) -> int:
    if not CONFIG.exists():
        print(f"No panel config at {CONFIG}; add the applet from Settings instead.")
        return 1

    raw = CONFIG.read_text()
    if app_id in raw:
        print(f"{app_id} is already on the panel.")
        return 0

    # The file is RON: Some(([ ...left... ], [ ...right... ])).
    marker = "], ["
    if marker not in raw:
        print("Unexpected panel config layout; add the applet from Settings instead.")
        return 1

    shutil.copyfile(CONFIG, CONFIG.with_suffix(".bak"))
    head, _, tail = raw.partition(marker)
    CONFIG.write_text(f'{head}{marker}\n    "{app_id}",{tail}')
    print(f"Added {app_id} to the panel. Log out and back in, or restart cosmic-panel.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "io.github.MoisesRoig.cosmic-ext-applet-agents"))
