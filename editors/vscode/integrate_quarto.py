"""Route the installed Quarto extension's text-document Preview controls to qlive.

Only contributed menus/keybindings change; the upstream command and code stay intact.
Run again after an upstream extension update, or pass --restore to undo integration.
"""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("extension", type=Path, help="Installed Quarto extension directory")
parser.add_argument("--restore", action="store_true")
args = parser.parse_args()
manifest = args.extension.resolve() / "package.json"
backup = manifest.with_name("package.json.qlive-backup")
if args.restore:
    if not backup.exists():
        raise SystemExit("No qlive backup exists")
    manifest.write_bytes(backup.read_bytes())
    print(f"Restored {manifest}; reload VS Code")
else:
    original = manifest.read_bytes()
    package = json.loads(original)
    if package.get("publisher") != "quarto" or package.get("name") != "quarto":
        raise SystemExit("Expected the quarto.quarto extension")
    # Always transform the pristine version so reruns cannot duplicate controls.
    if not backup.exists():
        backup.write_bytes(original)
    package = json.loads(backup.read_bytes())
    contributes = package["contributes"]
    target = "resourceScheme == file && (resourceExtname == .qmd || resourceExtname == .md)"
    changed = 0
    commands = {"quarto.preview": "qlive.start", "quarto.insertCodeCell": "qlive.insertCodeCell",
                "quarto.goToNextCell": "qlive.goToNextCell", "quarto.goToPreviousCell": "qlive.goToPreviousCell"}
    for entries in contributes.get("menus", {}).values():
        integrated = []
        for entry in entries:
            if entry.get("command") in commands:
                condition = entry.get("when", "true")
                integrated.append({**entry, "command": commands[entry["command"]], "when": f"({condition}) && ({target})"})
                integrated.append({**entry, "when": f"({condition}) && !({target})"})
                changed += 1
            else:
                integrated.append(entry)
        entries[:] = integrated
    for binding in list(contributes.get("keybindings", [])):
        if binding.get("command") in commands:
            contributes["keybindings"].append({**binding, "command": commands[binding["command"]], "when": f"({binding.get('when', 'true')}) && ({target})"})
            changed += 1
    manifest.write_text(json.dumps(package, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"Routed {changed} Preview contributions to qlive; backup: {backup}; reload VS Code")
