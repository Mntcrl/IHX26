#!/bin/sh
set -eu

if [ "$#" -ne 0 ]; then
    echo "Usage: $0 (serves only 127.0.0.1:8080; no bind options)" >&2
    exit 2
fi

root=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
if [ ! -f "$root/dist/index.html" ]; then
    echo "Missing dist/index.html next to $0. Build or unpack the production verifier." >&2
    exit 1
fi

exec python3 - "$root/dist" <<'PY'
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import sys


class VerifierHandler(SimpleHTTPRequestHandler):
    extensions_map = {
        **SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
        ".mjs": "text/javascript",
    }


handler = partial(VerifierHandler, directory=sys.argv[1])
with ThreadingHTTPServer(("127.0.0.1", 8080), handler) as server:
    print("Galaxy verifier: http://127.0.0.1:8080/ (Ctrl+C to stop)", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
PY
