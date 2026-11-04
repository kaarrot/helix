#!/usr/bin/env python3
"""Regenerate the main-kuba feature/rebuild README and merge maps using Python 3.

Run from any directory: python3 docs/merge-map/generate.py
The JSON is a review snapshot, not a live analysis of the current branches.
"""

import json
import math
from html import escape
from pathlib import Path


DIRECTORY = Path(__file__).resolve().parent
ROOT = DIRECTORY.parent.parent
PALETTES = {
    "light": {
        "surface": "#ffffff", "ink": "#1b1922", "ink2": "#4c4957",
        "muted": "#6c687a", "rule": "#e3e0ea",
        "heat": ["#a998ee", "#7d63e0", "#5634c4", "#33198a"],
    },
    "dark": {
        "surface": "#1c1a23", "ink": "#ebe8f2", "ink2": "#bdb8cb",
        "muted": "#9d98ad", "rule": "#2f2b3a",
        "heat": ["#5543ad", "#7461d9", "#a08ff2", "#d4ccfb"],
    },
}


def plural(count):
    return f"{count} file" + ("s" if count != 1 else "")


def severity(count):
    return 3 if count >= 10 else 2 if count >= 4 else 1 if count >= 2 else 0


def conflicting_files(pairs, step):
    return {file for (later, _), files in pairs.items() if later == step for file in files}


def svg(steps, pairs, palette):
    # These dimensions and colors follow the report's HTML table: 20px cells,
    # 2px spacing, a lower triangle, and the same initially selected pair.
    selected = (21, 20)
    selected_files = pairs[selected]
    grid_x, grid_y, pitch = 320, 76, 22
    divider_y = grid_y + (len(steps) - 1) * pitch + 16
    files_y = divider_y + 88
    column_length = math.ceil(len(selected_files) / 2)
    height = files_y + column_length * 20 + 12
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="820" height="{height}" '
        f'viewBox="0 0 820 {height}" role="img" aria-labelledby="title description">',
        '<title id="title">main-kuba pairwise merge conflict map</title>',
        '<desc id="description">Merge order from the 3 November 2026 review. '
        'Rows are steps 2 to 22; columns are earlier steps 1 to 21. '
        'Purple intensity represents 1, 2 to 3, 4 to 9, or 10 or more conflicting files. '
        'Outlined cells mean no conflict. The selected pair is step 21, '
        'feat/comment-hunk, with step 20, dev-3-way-diff-second-attempt: 17 files. '
        'The README provides feature descriptions, merge order, and conflict counts as text.</desc>',
        '<style>text{font-family:Recursive,ui-sans-serif,system-ui,sans-serif}'
        '.mono{font-family:Recursive,ui-monospace,Consolas,"Liberation Mono",monospace}'
        '</style>',
        f'<rect x="0.5" y="0.5" width="819" height="{height - 1}" rx="8" '
        f'fill="{palette["surface"]}" stroke="{palette["rule"]}"/>',
    ]

    def text(x, y, content, size=12, color="ink2", mono=False, bold=False, anchor="start"):
        class_attribute = ' class="mono"' if mono else ''
        parts.append(
            f'<text x="{x}" y="{y}" font-size="{size}" fill="{palette[color]}" '
            f'text-anchor="{anchor}" font-weight="{700 if bold else 400}"'
            f'{class_attribute}>{escape(str(content))}</text>'
        )

    def rect(x, y, size, fill, stroke="none", title=None):
        parts.append(f'<rect x="{x}" y="{y}" width="{size}" height="{size}" '
                     f'rx="3" fill="{fill}" stroke="{stroke}">')
        if title:
            parts.append(f'<title>{escape(title)}</title>')
        parts.append('</rect>')

    for x, label, color in zip(
        [16, 112, 224, 336, 478],
        ["1 file", "2–3 files", "4–9 files", "10 or more", "no conflict"],
        [*palette["heat"], palette["surface"]],
    ):
        rect(x, 17, 12, color, palette["muted"] if label == "no conflict" else "none")
        text(x + 20, 28, label, size=13)

    text(16, grid_y - 10, "step", size=11, color="muted", mono=True)
    for column in range(1, len(steps)):
        text(grid_x + (column - 1) * pitch + 10, grid_y - 10, column,
             size=11, color="ink" if column == selected[1] else "muted",
             mono=True, bold=column == selected[1], anchor="middle")

    for step in range(2, len(steps) + 1):
        y = grid_y + (step - 2) * pitch
        text(16, y + 14, step, color="muted", mono=True, bold=step == selected[0])
        text(46, y + 14, steps[step - 1], mono=True,
             color="ink" if step == selected[0] else "ink2", bold=step == selected[0])
        for earlier in range(1, step):
            files = pairs.get((step, earlier), [])
            description = (f'Step {step} {steps[step - 1]} with step {earlier} '
                           f'{steps[earlier - 1]}: {plural(len(files))} in conflict')
            rect(grid_x + (earlier - 1) * pitch, y, 20,
                 palette["heat"][severity(len(files))] if files else palette["surface"],
                 "none" if files else palette["rule"],
                 description + ("\n" + "\n".join(files) if files else ""))

    x = grid_x + (selected[1] - 1) * pitch
    y = grid_y + (selected[0] - 2) * pitch
    rect(x - 4, y - 4, 28, palette["ink"])
    rect(x - 2, y - 2, 24, palette["surface"])
    rect(x, y, 20, palette["heat"][severity(len(selected_files))])
    parts.append(f'<path d="M16 {divider_y}H804" stroke="{palette["rule"]}"/>')
    text(16, divider_y + 36, f"{plural(len(selected_files))} in conflict",
         size=24, color="ink", bold=True)
    text(16, divider_y + 62,
         f"Step 21 {steps[20]} with step 20 {steps[19]}", size=13, mono=True)
    for index, file in enumerate(selected_files):
        text(16 + (index // column_length) * 400,
             files_y + (index % column_length) * 20, file, size=12, mono=True)
    parts.append('</svg>')
    return "\n".join(parts) + "\n"


def readme(steps, pairs, data):
    lines = [
        '<!-- Generated by python3 docs/merge-map/generate.py from data.json. -->',
        '# main-kuba', '',
        'The feature branches in this fork and the recommended path to rebuild them',
        'on a fresh `main` branch. The plan combines **22 branch merges** with folding',
        'and a final regression-test cherry-pick.', '',
        '[Rebuild path](#rebuild-path) · [Conflict map](#conflict-map) ·',
        '[Features and merge order](#features-and-merge-order) · [Build](#build)', '',
        '## Rebuild path', '',
        'Follow these phases in order:', '',
        '| Phase | Steps | What to expect |',
        '| --- | --- | --- |',
        '| Clean merges | 1–11 | Configuration, editing, performance, previews, themes, and encryption. |',
        '| Small conflicts | 12–16 | Search, picker layout, cursorline, reload, and completion; 1–3 files per step. |',
        '| Larger features | 17–22 | Streaming, paths, sorting, diff/merge, reviews, and debugger; 1–32 files per step. |',
        '| Finish | 23–24 | Apply folding and optionally retain the spinner regression test. |', '',
        'The step numbers are shared by the feature list and conflict map.', '',
        '## Conflict map', '',
        'Rows are merge steps; column numbers refer to earlier steps. The purple',
        'scale shows how many files conflicted in each pair. The selected pair is',
        'step 21 (`feat/comment-hunk`) against step 20 (three-way diff).', '',
        '<picture>',
        '  <source media="(prefers-color-scheme: dark)" srcset="docs/merge-map/map-dark.svg">',
        '  <img alt="Pairwise conflict map for 22 feature branches, in merge order. '
        'The largest overlap is 17 files between steps 21 and 20. '
        'The feature list below shows the full merge order and conflict counts." '
        'src="docs/merge-map/map-light.svg">',
        '</picture>', '',
        '## Features and merge order', '',
        'Merge one branch at a time in this order.',
        'Resolve and check each step before moving to the next.', '',
        '| Step | Feature | Branch or cherry-pick | Distinct conflicting files |',
        '| --- | --- | --- | --- |',
    ]
    for step, branch in enumerate(steps, 1):
        count = len(conflicting_files(pairs, step))
        feature = data['features'][step - 1]
        lines.append(f'| {step} | {feature} | `{branch}` | {count if count else "0 (clean)"} |')
    lines.extend([
        '| 23 | Folding fitted to the combined features. | `git cherry-pick 452a37af1 2db61bd42` | Not measured in the pairwise map |',
        '| 24 | Optional spinner regression test. | `git cherry-pick 98206ee63` | Not measured in the pairwise map |',
        '',
        'Counts are distinct files across a step\'s earlier conflicting pairs, not the',
        'sum of its cells. Steps 23 and 24 are cherry-picks and have no matrix rows.', '',
        '## Build', '',
        'The `pgp` feature requires Rust 1.88 or newer.', '',
        '```sh',
        'HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo build --release --features pgp',
        '```',
    ])
    return '\n'.join(lines) + '\n'


def main():
    data = json.loads((DIRECTORY / 'data.json').read_text())
    steps = data['steps']
    if len(data['features']) != len(steps):
        raise ValueError('Each merge step needs a feature description')
    pairs = {}
    for pair in data['pairs']:
        key = (pair['step'], pair['earlier_step'])
        if not (1 <= key[1] < key[0] <= len(steps)) or key in pairs or not pair['files']:
            raise ValueError(f'Invalid or duplicate conflict pair: {key}')
        pairs[key] = pair['files']

    for theme, palette in PALETTES.items():
        (DIRECTORY / f'map-{theme}.svg').write_text(svg(steps, pairs, palette))

    (ROOT / 'README.md').write_text(readme(steps, pairs, data))
    print(f'Generated two SVGs and the README from {len(steps)} branches and {len(pairs)} pairs.')


if __name__ == '__main__':
    main()
