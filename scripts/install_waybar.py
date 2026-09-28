#!/usr/bin/env python3
"""Add the recorder to existing Waybar bars, backing up every changed file."""

import datetime
import json
import os
from pathlib import Path
import re
import shlex
import shutil


def load_jsonc(text):
    # Match quoted strings first, so URLs and escaped quotes remain untouched.
    tokens = r'"(?:\\.|[^"\\])*"|//[^\n]*|/\*[\s\S]*?\*/'
    text = re.sub(tokens, lambda m: m[0] if m[0].startswith('"') else ' ', text)
    text = re.sub(r'"(?:\\.|[^"\\])*"|,\s*(?=[}\]])',
                  lambda m: m[0] if m[0].startswith('"') else '', text)
    return json.loads(text)


def install():
    root = Path(__file__).resolve().parent.parent
    home = Path.home()
    config_home = Path(os.environ.get('XDG_CONFIG_HOME', home / '.config'))
    data_home = Path(os.environ.get('XDG_DATA_HOME', home / '.local/share'))
    directory = config_home / 'waybar'
    config = next((directory / name for name in ('config.jsonc', 'config')
                   if (directory / name).is_file()), None)
    if config is None:
        raise SystemExit('No existing Waybar config found; create a bar first.')
    binary = home / '.local/bin/mentarch-recorder'
    menu = data_home / 'mentarch-recorder/waybar-menu.ui'
    if not binary.is_file() or not menu.is_file():
        raise SystemExit('Run sh scripts/install.sh first.')
    if not shutil.which('systemd-run'):
        raise SystemExit('systemd-run is required to keep recording across bar reloads.')
    content = load_jsonc(config.read_text())
    bars = content if isinstance(content, list) else [content]
    snippet = directory / 'mentarch-recorder.json'
    for bar in bars:
        if not isinstance(bar, dict):
            raise SystemExit('Expected each Waybar configuration to be an object.')
        modules = bar.setdefault('modules-right', [])
        if 'custom/recorder' not in modules:
            modules.insert(0, 'custom/recorder')
        includes = bar.get('include', [])
        if isinstance(includes, str):
            includes = [includes]
        if str(snippet) not in includes:
            includes.append(str(snippet))
        bar['include'] = includes

    def action(name):
        return shlex.join(['systemd-run', '--user', '--collect', '--quiet',
                           '--description=Window Recorder', '--', str(binary), f'--{name}'])

    module = {'custom/recorder': {
        'exec': shlex.join([str(binary), '--waybar']),
        'return-type': 'json', 'interval': 1, 'format': '{text}',
        'exec-on-event': False,
        'on-click': action('toggle'), 'on-click-middle': action('open'),
        'menu': 'on-click-right', 'menu-file': str(menu),
        'menu-actions': {name: action(name) for name in
                         ('start', 'stop', 'cancel', 'open', 'folder', 'show', 'quit')},
    }}
    style = directory / 'style.css'
    old_style = style.read_text() if style.exists() else ''
    css_import = '@import url("mentarch-recorder.css");'
    updated_style = old_style if css_import in old_style else css_import + '\n' + old_style
    updates = {
        snippet: json.dumps(module, indent=2, ensure_ascii=False) + '\n',
        directory / 'mentarch-recorder.css': (root / 'data/waybar.css').read_text(),
        style: updated_style,
        config: json.dumps(content, indent=2, ensure_ascii=False) + '\n',
    }
    stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S-%f')
    for path, text in updates.items():
        if path.exists() and path.read_text() == text:
            continue
        if path.exists():
            backup = path.with_name(path.name + '.before-recorder-' + stamp)
            shutil.copy2(path, backup)
            print(f'Backed up {backup}')
        temporary = path.with_name(path.name + '.recorder.tmp')
        temporary.write_text(text)
        temporary.replace(path)
        print(f'Updated {path}')
    print('Recorder widget installed. Reload Waybar: pkill -USR2 -x waybar')


if __name__ == '__main__':
    install()
