#!/usr/bin/env bash
# Reproducible, text-only AutoJev development server for an RTX 3090.
set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
LOCAL="$ROOT/.local/jev"
SOURCE="$LOCAL/jev.cpp"
SERVER="$SOURCE/build/bin/llama-server"
MODEL="$LOCAL/autojev-Q4_K_M.gguf"
CONFIG="$LOCAL/forkdeck.toml"
SERVER_REV=aa885f0a86efe36079d70b522f927cd2fc38683b
MODEL_REV=834f2eac1702c7e47e12d63eb729297951362b8a
MODEL_SHA=47f0f73a74894883bd2ec62a893a40fe334880af95088a6b019c016c2b7c98b1
export FORKDECK_JEV_KEY=${FORKDECK_JEV_KEY:-forkdeck-local-dev}

fail() { echo "local-jev: $*" >&2; exit 1; }
require() { command -v "$1" >/dev/null || fail "Missing prerequisite: $1"; }
ready() {
    [[ -x "$SERVER" && -f "$MODEL" && -f "$CONFIG" ]] || fail "Run $0 setup first."
}

case "${1:-help}" in
    setup)
        for tool in git curl cmake nvcc g++ sha256sum; do require "$tool"; done
        mkdir -p "$LOCAL"
        if [[ ! -d "$SOURCE/.git" ]]; then
            git init "$SOURCE"
            git -C "$SOURCE" remote add origin https://github.com/thomasgauthier/jev.cpp.git
        fi
        [[ -z "$(git -C "$SOURCE" status --porcelain --untracked-files=no)" ]] || fail "Server checkout has local changes: $SOURCE"
        if [[ "$(git -C "$SOURCE" rev-parse HEAD 2>/dev/null || true)" != "$SERVER_REV" ]]; then
            git -C "$SOURCE" fetch --depth 1 origin "$SERVER_REV"
            git -C "$SOURCE" checkout --detach "$SERVER_REV"
        fi
        cmake -S "$SOURCE" -B "$SOURCE/build" \
            -DCMAKE_BUILD_TYPE=Release -DGGML_CUDA=ON -DCMAKE_CUDA_ARCHITECTURES=86 \
            -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF \
            -DLLAMA_BUILD_UI=OFF -DLLAMA_USE_PREBUILT_UI=OFF
        cmake --build "$SOURCE/build" --target llama-server -j "${JEV_BUILD_JOBS:-4}"

        # Resume interrupted downloads; promote only after verifying the published LFS hash.
        if [[ ! -f "$MODEL" ]]; then
            # A new curl process recalculates the resume offset after a truncated transfer.
            for attempt in 1 2 3 4 5; do
                if curl --http1.1 --fail --location --retry 2 --continue-at - \
                    --connect-timeout 30 --speed-limit 1024 --speed-time 120 \
                    "https://huggingface.co/thomasgauthier/autojev-27b-GGUF/resolve/$MODEL_REV/autojev-Q4_K_M.gguf" \
                    --output "$MODEL.part"; then
                    break
                fi
                [[ "$attempt" != 5 ]] || fail "Download interrupted; rerun setup to resume $MODEL.part"
                echo "Download interrupted; resuming (attempt $((attempt + 1))/5)..." >&2
                sleep 2
            done
            printf '%s  %s\n' "$MODEL_SHA" "$MODEL.part" | sha256sum --check
            mv -- "$MODEL.part" "$MODEL"
        else
            printf '%s  %s\n' "$MODEL_SHA" "$MODEL" | sha256sum --check
        fi
        cat > "$CONFIG" <<'TOML'
[router]
kind = "jev"

[router.jev]
base_url = "http://127.0.0.1:8000"
model = "jev-latest"
api_key_env = "FORKDECK_JEV_KEY"
timeout_ms = 60000

[automation]
enabled = false
TOML
        echo "Setup complete. Start with: $0 start"
        ;;
    start)
        ready
        require nvidia-smi
        gpu=$(nvidia-smi --id=0 --query-gpu=name --format=csv,noheader) || fail "GPU access unavailable. Run in a GPU-enabled terminal outside the sandbox."
        [[ "$gpu" == *"RTX 3090"* ]] || fail "Expected RTX 3090 at GPU 0, found: $gpu"
        export CUDA_VISIBLE_DEVICES=0
        exec "$SERVER" --model "$MODEL" --system-one --n-gpu-layers 99 --fit off \
            --verbosity 4 --batch-size 512 --ubatch-size 512 \
            --ctx-size 8192 --parallel 1 --host 127.0.0.1 --port 8000 \
            --api-key "$FORKDECK_JEV_KEY"
        ;;
    test)
        ready
        require cargo
        require python3
        cd "$ROOT"
        cargo build --locked
        # cargo metadata also honors CARGO_TARGET_DIR and local Cargo configuration.
        target_dir=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
        python3 - "$target_dir/debug/forkdeck" "$CONFIG" <<'PY'
import json
import os
import socket
import subprocess
import sys
import tempfile
from pathlib import Path

binary, config = sys.argv[1:]

def route(task, config_path=config, key=None):
    env = os.environ.copy()
    if key is not None:
        env["FORKDECK_JEV_KEY"] = key
    result = subprocess.run(
        [binary, "--config", config_path, "route", task],
        env=env, text=True, capture_output=True, timeout=75, check=True,
    )
    return json.loads(result.stdout), result.stderr

for task in (
    "Fix a typo in the README heading",
    "Fix the pagination bug: page two repeats the last item from page one; add a regression test",
    "Design a cross-service database migration with concurrent writes and a safe rollback strategy",
):
    decision, diagnostics = route(task)
    assert decision["source"] == "jev", (decision, diagnostics)
    assert "rules used instead" not in diagnostics, diagnostics
    assert len(decision["gates"]) == 3, decision
    print(f'PASS {decision["ms"]} ms: {decision["agent"]} / {decision["tier"]} / review={decision["review"]}: {task}', flush=True)

decision, diagnostics = route("Fix a typo", key=os.environ["FORKDECK_JEV_KEY"] + "-invalid")
assert decision["source"] == "rules", (decision, diagnostics)
assert "401" in diagnostics and "rules used instead" in diagnostics, diagnostics
print("PASS invalid key: explicit 401 and rules fallback", flush=True)

# Reserve a local port without listening, so no unrelated service receives the request.
with socket.socket() as reserved, tempfile.TemporaryDirectory(prefix="forkdeck-jev-") as tmp:
    reserved.bind(("127.0.0.1", 0))
    port = reserved.getsockname()[1]
    offline = Path(tmp) / "offline.toml"
    offline.write_text(Path(config).read_text().replace("127.0.0.1:8000", f"127.0.0.1:{port}"))
    decision, diagnostics = route("Fix a typo", str(offline))
    assert decision["source"] == "rules", (decision, diagnostics)
    assert "cannot connect" in diagnostics and "rules used instead" in diagnostics, diagnostics
print("PASS unavailable server: explicit connection error and rules fallback", flush=True)
PY
        ;;
    help|--help|-h)
        echo "Usage: $0 {setup|start|test}"
        echo "setup: build pinned CUDA server and download/verify Q4_K_M model (~15.8 GB)"
        echo "start: serve on RTX 3090, 127.0.0.1:8000; Ctrl-C to stop"
        echo "test: check live Forkdeck routing, invalid authentication, and offline fallback"
        echo "Optional: FORKDECK_JEV_KEY (same in both terminals), JEV_BUILD_JOBS (default 4)"
        ;;
    *) fail "Unknown command: $1 (use --help)" ;;
esac
