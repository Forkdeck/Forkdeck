#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
flatc=$("$root/scripts/flatc.sh")
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/rust" "$tmp/ts"
"$flatc" --rust -o "$tmp/rust" "$root/web/schema/board.fbs"
"$flatc" --ts --ts-entry-points -o "$tmp/ts" "$root/web/schema/board.fbs"
# flatc 25.9.23 emits .js imports and a TS enum. Use explicit .ts imports and
# the equivalent bidirectional const map so Node 24 type stripping can load it.
python3 - "$tmp/ts" <<'PY'
import pathlib, re, sys
for p in pathlib.Path(sys.argv[1]).rglob('*.ts'):
    s = p.read_text().replace(".js'", ".ts'")
    def enum(m):
        pairs = re.findall(r'(\w+)\s*=\s*(\d+)', m[2])
        fields = [f'  {k}: {v}, {v}: "{k}"' for k, v in pairs]
        return 'export const '+m[1]+' : { readonly [key: number]: string } & { '+ '; '.join(f'readonly {k}: {v}' for k, v in pairs) +' } = {\n'+',\n'.join(fields)+'\n} as const;\nexport type '+m[1]+' = '+ 'number'+';'
    s = re.sub(r'export enum (\w+) \{([^}]+)\}', enum, s)
    p.write_text(s)
PY
if [[ ${1:-} == --check ]]; then
    diff -ru "$root/src/server/wire" "$tmp/rust"
    diff -ru "$root/web/src/wire" "$tmp/ts"
else
    mkdir -p "$root/src/server/wire" "$root/web/src/wire"
    cp -R "$tmp/rust/." "$root/src/server/wire/"
    cp -R "$tmp/ts/." "$root/web/src/wire/"
fi
