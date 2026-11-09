"""Pinned build-time script conversions shared by all Meowpuut platforms."""
from functools import lru_cache
import gzip
import hashlib
import json
from pathlib import Path

SOURCE = Path(__file__).resolve().parent.parent / 'DataSources/MimoNormalization'


def _verified(path, expected):
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != expected:
        raise ValueError(f'Normalization source checksum mismatch: {path.name}')
    return data


@lru_cache(maxsize=1)
def _mappings():
    manifest = json.loads((SOURCE / 'SOURCE.json').read_text(encoding='utf-8'))
    if manifest.get('format_version') != 1 or manifest.get('icu_version') != '78.2':
        raise ValueError('Unsupported normalization snapshot version')
    _verified(SOURCE / 'ICU-LICENSE.txt', manifest['license_sha256'])
    _verified(SOURCE / 'ICU-Hans_Hant.txt', manifest['rule_file_sha256'])
    snapshot = _verified(SOURCE / manifest['snapshot_file'], manifest['snapshot_sha256'])
    content = gzip.decompress(snapshot)
    if hashlib.sha256(content).hexdigest() != manifest['snapshot_content_sha256']:
        raise ValueError('Normalization snapshot content checksum mismatch')
    values = {}
    for number, row in enumerate(content.decode('utf-8').splitlines(), 1):
        fields = row.split('\t')
        if len(fields) != 3 or fields[0] not in {'s2t', 't2s'} or not fields[1]:
            raise ValueError(f'Invalid normalization snapshot row {number}')
        key = (fields[0], fields[1])
        if key in values:
            raise ValueError(f'Duplicate normalization snapshot row {number}')
        values[key] = fields[2]
    overrides = _verified(SOURCE / manifest['overrides_file'], manifest['overrides_sha256'])
    seen = set()
    for number, row in enumerate(overrides.decode('utf-8').splitlines(), 1):
        fields = row.split('\t')
        if len(fields) != 3 or fields[0] not in {'s2t', 't2s'}:
            raise ValueError(f'Invalid normalization override row {number}')
        key = (fields[0], fields[1])
        if key in seen or key not in values or values[key] == fields[2]:
            raise ValueError(f'Duplicate, missing or redundant normalization override row {number}')
        seen.add(key)
        values[key] = fields[2]
    return values


def convert(words, traditional=False):
    values = _mappings()
    mode = 's2t' if traditional else 't2s'
    result = []
    for word in words:
        key = (mode, word)
        if key not in values:
            raise ValueError(
                f'Normalization snapshot missing {mode} mapping for {word!r}. '
                'Review and update it as described in DataSources/MimoNormalization/README.md.'
            )
        result.append(values[key])
    return result
