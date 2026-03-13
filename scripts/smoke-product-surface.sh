#!/bin/sh
set -eu

BINARY="${1:-target/debug/yacli}"

if [ ! -x "$BINARY" ]; then
    echo "Binary is not executable: $BINARY" >&2
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "python3 is required for smoke-product-surface.sh" >&2
    exit 1
fi

tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/yacli-smoke.XXXXXX")"
cleanup() {
    if [ -n "${http_pid:-}" ]; then
        kill "$http_pid" >/dev/null 2>&1 || true
        wait "$http_pid" 2>/dev/null || true
    fi
    rm -rf "$tmp_root"
}
trap cleanup EXIT INT TERM HUP

pick_port() {
    python3 - <<'PY'
import socket

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
}

current_target() {
    system="$(uname -s)"
    machine="$(uname -m)"
    case "${system}:${machine}" in
        Linux:x86_64) printf '%s\n' "x86_64-unknown-linux-gnu" ;;
        Linux:aarch64|Linux:arm64) printf '%s\n' "aarch64-unknown-linux-gnu" ;;
        Darwin:x86_64) printf '%s\n' "x86_64-apple-darwin" ;;
        Darwin:arm64|Darwin:aarch64) printf '%s\n' "aarch64-apple-darwin" ;;
        *)
            echo "unsupported host target for smoke-product-surface.sh: ${system}:${machine}" >&2
            exit 1
            ;;
    esac
}

printf '==> yacli version/help surface\n'
"$BINARY" --version >/dev/null
"$BINARY" mcp --help >/dev/null
"$BINARY" mcp install --help >/dev/null
"$BINARY" update --help >/dev/null
"$BINARY" --format json guide --topic mail >/dev/null

printf '==> stdio MCP initialize (JSONL)\n'
python3 - "$BINARY" <<'PY'
import json
import subprocess
import sys

binary = sys.argv[1]
proc = subprocess.Popen(
    [binary, "mcp"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
)
request = {
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
        "protocolVersion": "2025-11-25",
        "capabilities": {
            "experimental": {
                "io.modelcontextprotocol/ui": {
                    "mimeTypes": ["text/html;profile=mcp-app"],
                    "resourceTemplates": True,
                }
            }
        },
        "clientInfo": {"name": "smoke", "version": "0.1.0"},
    },
}
proc.stdin.write(json.dumps(request) + "\n")
proc.stdin.flush()
line = proc.stdout.readline()
if not line:
    stderr = proc.stderr.read()
    raise SystemExit(f"stdio initialize returned no output: {stderr}")
payload = json.loads(line)
assert payload["result"]["protocolVersion"] == "2025-11-25", payload
assert payload["result"]["serverInfo"]["name"] == "yacli", payload
proc.terminate()
proc.wait(timeout=5)
PY

printf '==> HTTP MCP initialize + tools/list\n'
http_port="$(pick_port)"
http_addr="127.0.0.1:${http_port}"
"$BINARY" mcp --transport http --listen "$http_addr" >/dev/null 2>&1 &
http_pid="$!"
python3 - "$http_addr" <<'PY'
import socket
import sys
import time

host, port = sys.argv[1].split(":")
port = int(port)
deadline = time.time() + 10
while time.time() < deadline:
    sock = socket.socket()
    sock.settimeout(0.5)
    try:
        sock.connect((host, port))
    except OSError:
        time.sleep(0.1)
    else:
        sock.close()
        raise SystemExit(0)
    finally:
        sock.close()
raise SystemExit("HTTP MCP server did not become ready in time")
PY
python3 - "http://${http_addr}/mcp" <<'PY'
import json
import sys
import urllib.request

url = sys.argv[1]
initialize = urllib.request.Request(
    url,
    data=json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "experimental": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": ["text/html;profile=mcp-app"],
                            "resourceTemplates": True,
                        }
                    }
                },
                "clientInfo": {"name": "smoke-http", "version": "0.1.0"},
            },
        }
    ).encode("utf-8"),
    headers={"Content-Type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(initialize) as response:
    session_id = response.headers["Mcp-Session-Id"]
    payload = json.load(response)
assert session_id, "missing Mcp-Session-Id header"
assert payload["result"]["protocolVersion"] == "2025-11-25", payload

tools_list = urllib.request.Request(
    url,
    data=json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {},
        }
    ).encode("utf-8"),
    headers={
        "Content-Type": "application/json",
        "Mcp-Session-Id": session_id,
    },
    method="POST",
)
with urllib.request.urlopen(tools_list) as response:
    payload = json.load(response)
tools = payload["result"]["tools"]
assert any(tool["name"] == "yacli.app.snapshot" for tool in tools), payload
assert any(tool["name"] == "yacli.mail.send" for tool in tools), payload
PY

printf '==> update check against local release mirror\n'
target="$(current_target)"
mirror_root="${tmp_root}/mirror"
version="v9.9.9"
asset_name="yacli-${target}.tar.gz"
mkdir -p "${mirror_root}/releases/download/${version}"
printf 'deadbeef  %s\n' "$asset_name" > "${mirror_root}/releases/download/${version}/SHA256SUMS"
mirror_port="$(pick_port)"
python3 -m http.server "$mirror_port" --bind 127.0.0.1 --directory "$mirror_root" >/dev/null 2>&1 &
mirror_pid="$!"
sleep 1
update_output="$("$BINARY" update --check --base-url "http://127.0.0.1:${mirror_port}/releases/download/${version}")"
kill "$mirror_pid" >/dev/null 2>&1 || true
wait "$mirror_pid" 2>/dev/null || true
mirror_pid=""
python3 - <<'PY' "$update_output"
import json
import sys

payload = json.loads(sys.argv[1])
assert payload["operation"] == "update.check", payload
assert payload["status"] == "update_available", payload
assert payload["target_version"] == "9.9.9", payload
assert payload["requested_version"] == "latest", payload
assert "/releases/download/v9.9.9" in payload["base_url"], payload
PY

printf 'Smoke product surface passed for %s\n' "$BINARY"
