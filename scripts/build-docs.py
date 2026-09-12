#!/usr/bin/env python3
"""Build both documentation tracks into one Pages artifact."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--human-source', type=Path, help='human checkout (including local changes)')
parser.add_argument('--human-ref', default='main-human', help='local Git ref used without --human-source')
parser.add_argument('--output', type=Path, default=ROOT / 'dist', help='generated site directory')
args = parser.parse_args()
final = args.output.resolve()
if final == ROOT or ROOT.is_relative_to(final) or final.is_relative_to(ROOT / 'docs'):
    raise SystemExit('Output must not replace the repository or documentation sources.')
base = os.environ.get('MDBOOK_OUTPUT__HTML__SITE_URL', '/').rstrip('/') + '/'
if not base.startswith('/') or base.startswith('//'):
    raise SystemExit('Site URL must be a root-relative path, such as /Rustmux/.')

with tempfile.TemporaryDirectory(prefix='rustmux-docs-') as directory:
    staging = Path(directory)
    human = staging / 'human'
    if args.human_source:
        shutil.copytree(args.human_source.resolve(), human,
                        ignore=shutil.ignore_patterns('.git', 'target', 'dist'))
    else:
        archive = staging / 'human.tar'
        with archive.open('wb') as output:
            subprocess.run(['git', 'archive', args.human_ref], cwd=ROOT, stdout=output, check=True)
        human.mkdir()
        with tarfile.open(archive) as contents:
            contents.extractall(human, filter='data')
    if not (human / 'book.toml').is_file():
        raise SystemExit('Human documentation is missing. Use --human-source PATH for a candidate checkout.')

    # Presentation is shared; each branch owns its content and book configuration.
    shutil.copytree(ROOT / 'docs/theme', human / 'docs/theme', dirs_exist_ok=True)
    book = human / 'book.toml'
    book.write_text(book.read_text().replace('[output.html]',
        '[output.html]\nadditional-css = ["docs/theme/rustmux.css"]\n'
        'additional-js = ["docs/theme/rustmux.js"]', 1))
    output = staging / 'site'
    tracks = [('main', ROOT, output, base),
              ('main-human', human, output / 'main-human', base + 'main-human/')]
    pages = {}
    for track, source, destination, site_url in tracks:
        env = dict(os.environ, MDBOOK_OUTPUT__HTML__SITE_URL=site_url)
        env['MDBOOK_OUTPUT__HTML__EDIT_URL_TEMPLATE'] = (
            f'https://github.com/Jacky-Lzx/Rustmux/edit/{track}/{{path}}')
        subprocess.run(['mdbook', 'build', str(source), '--dest-dir', str(destination)], env=env, check=True)
        if not (destination / 'index.html').is_file() or not list(destination.glob('searchindex-*.js')):
            raise SystemExit(f'Incomplete documentation build: {track}')
        pages[track] = sorted(str(p.relative_to(destination)) for p in destination.rglob('*.html'))

    shutil.copyfile(ROOT / 'docs/theme/track-switcher.js', output / 'track-switcher.js')
    for track, _, destination, _ in tracks:
        for page_path in pages[track]:
            page = destination / page_path
            # Absolute site paths also work when a 404 is served at an unknown depth.
            site_root = base
            metadata = json.dumps({'track': track, 'page': page_path, 'root': site_root,
                                   'pages': pages}).replace('<', '\\u003c')
            injection = (f'<script id="rustmux-tracks" type="application/json">{metadata}</script>'
                         f'<script defer src="{site_root}track-switcher.js"></script>')
            page.write_text(page.read_text().replace('</body>', injection + '\n</body>'))
    # Do not replace the previous artifact until both builds succeed.
    if final.exists():
        shutil.rmtree(final)
    shutil.copytree(output, final)
print(f'Built main and main-human documentation in {final}.')
