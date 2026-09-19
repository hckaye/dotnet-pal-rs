"""Materialize the owner's requested, data-only platform refactor on its fixed base.

All source recipes are plaintext JSON. No recipe is executed as Python or shell.
Every output blob and the complete resulting Git tree must match the local patch.
"""
import hashlib
import json
from pathlib import Path, PurePosixPath
import subprocess

BASE = '83ec535f7776aa9ab5fa79350bf456965bde028a'
BASE_TREE = '2e50efcaccbf772e59bdd540d11ba46b5efc1fc2'
EXPECTED_TREE = '94ad1496b05c850d120b3e81e31726d59b22e8db'
INPUTS = Path('.refactor-inputs')
WORKFLOW = Path('.github/workflows/submit-platform-refactor.yml')

def git(*args):
    return subprocess.check_output(['git', *args])

def path(value):
    p = PurePosixPath(value)
    if p.is_absolute() or not p.parts or any(x in ('..', '.git') for x in p.parts):
        raise ValueError('invalid repository path')
    if str(p) != value:
        raise ValueError('non-canonical repository path')
    return Path(value)

assert not git('status', '--porcelain'), 'checkout is not clean'
assert git('rev-parse', BASE + '^{tree}').decode().strip() == BASE_TREE
meta = json.loads((INPUTS / 'meta.json').read_text())
assert meta['base_commit'] == BASE and meta['base_tree'] == BASE_TREE
assert meta['target_tree'] == EXPECTED_TREE
records = []
for i in range(1, 11):
    records.extend(json.loads((INPUTS / f'part-{i:02}.json').read_text()))
assert len(records) == 236
assert len({r['path'] for r in records}) == len(records)
replacements = {r['path']: r for r in meta['replace']}
assert set(replacements) == {'scripts/unwind-cache.sh'}
records = [replacements.get(r['path'], r) for r in records]
assert not set(meta['delete']) & {r['path'] for r in records}
assert len(meta['delete']) == 79
original = {}
outputs = []
for item in records:
    dest = path(item['path'])
    if 'from' in item:
        src = str(path(item['from']))
        if src not in original:
            original[src] = git('show', f'{BASE}:{src}').decode('utf-8')
    if 'text' in item:
        text = item['text']
    elif 'spans' in item:
        lines = original[item['from']].splitlines(keepends=True)
        chunks = []
        for span in item['spans']:
            if isinstance(span, str):
                chunks.append(span)
            else:
                lo, hi = span
                assert isinstance(lo, int) and isinstance(hi, int) and 0 <= lo <= hi <= len(lines)
                chunks.append(''.join(lines[lo:hi]))
        text = ''.join(chunks)
    else:
        text = original[item['from']]
    data = text.encode('utf-8')
    digest = hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest()
    assert digest == item['sha'], (str(dest), digest, item['sha'])
    assert item['mode'] in ('100644', '100755')
    outputs.append((dest, data, item['mode']))
for dest, data, mode in outputs:
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_bytes(data)
    dest.chmod(0o755 if mode == '100755' else 0o644)
for old in meta['delete']:
    path(old).unlink()
# Staging files are never part of the submitted code tree.
for staged in INPUTS.iterdir():
    assert staged.is_file() and not staged.is_symlink()
    staged.unlink()
INPUTS.rmdir()
WORKFLOW.unlink()
git('add', '-A')
actual = git('write-tree').decode().strip()
assert actual == EXPECTED_TREE, actual
print('Verified 236 file outputs and complete target tree:', actual)
