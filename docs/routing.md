[Documentation](README.md) · [Front page](../README.md)

# The fork layer

Two routers ship, and a default that picks between them:

- `auto`, the default: asks the decision server at `router.jev.base_url` when
  one answers, and uses the rules quietly when none is running. On this machine
  without `FORKDECK_JEV_KEY` set it uses the key `scripts/local-jev.sh` serves
  with, so starting that server is all it takes.
- `rules`: keyword and phrase lists from the config. No network. A keyword may
  be a phrase ("race condition", "stack trace"), and word forms match, so "fix"
  also matches "fixes", "fixed" and "fixing". Phrases such as "how should" or
  "pros and cons" in a typed task ask for a choice of approaches first. The
  built-in lists cover common phrasings; a list set in `[router.rules]`
  replaces the built-in one.
- `jev`: any server that speaks the Jev wire format (`POST /v1/systemone` with
  `choice`, `score` and `noul` questions). If it does not answer, the rules
  router is used and the interface says so.

Before either is asked, **route memory** looks at earlier jobs in the same
repository (or directory, for a typed task) and of the same kind. When at least
two tasks much like this one were routed by the decision server and went well
(done at the first try, with the review or the checks passing), and none like it
went badly, the task is routed the same way without asking. The router line says
`memory` and which jobs it followed; `forkdeck route "<task>"` shows the same.
Once a kind of task has gone well three times at a tier, the next one tries a
tier lower to spend fewer tokens. If that worker fails, it goes back to the
usual tier once, and that kind of task is not stepped down again for 30 days.
Only hashed words are kept, never the task's text. `[router.memory]` tunes it,
and `enabled = false` turns it off.

## Running the decision model locally

For a local RTX 3090, the repository includes a repeatable setup for
[AutoJev-27B Q4_K_M](https://huggingface.co/thomasgauthier/autojev-27b-GGUF).
It uses the [jev.cpp](https://github.com/thomasgauthier/jev.cpp) fork, which
preserves AutoJev's classifier head and serves `/v1/systemone`. This model
requires that fork; the publisher does not support standard llama.cpp or
LM Studio for it.

Requirements: Linux/WSL with GPU access, an RTX 3090 at GPU index 0, the CUDA
toolkit (`nvcc`), CMake, a C++ compiler, Git, curl, and `sha256sum`. Testing
also needs Rust/Cargo and Python 3. Allow at least 25 GB of free disk space;
the text model download is approximately 15.8 GB. No vision projector is needed.

```sh
./scripts/local-jev.sh setup   # build pinned CUDA source; download and verify weights
./scripts/local-jev.sh start   # foreground server; Ctrl-C stops it
```

In a second terminal, from this repository:

```sh
./scripts/local-jev.sh test

# Or route your own task (this does not launch coding agents):
export FORKDECK_JEV_KEY=forkdeck-local-dev
cargo run --locked -- --config .local/jev/forkdeck.toml route "Fix the pagination bug"
```

The script keeps source, builds, model weights, and its dedicated configuration
in ignored `.local/jev/`. Repeating `setup` reuses the build and verified model;
interrupted downloads resume. It never writes your normal Forkdeck configuration.
The server binds to `127.0.0.1:8000`, uses one request slot and an 8,192-token
context, and requests full GPU offload. The test configuration allows 60 seconds
per routing request and disables automation. The smoke test requires real Jev
answers for three tasks, then checks authentication failure and connection
failure both produce explicit rules-fallback diagnostics.

The development key defaults to `forkdeck-local-dev`. To change it, export
`FORKDECK_JEV_KEY` with the same value in both terminals. Set `JEV_BUILD_JOBS`
to change build concurrency (default 4). If `nvidia-smi` reports GPU access
blocked, run from a GPU-enabled terminal outside the sandbox. A working driver
alone is not sufficient to build: `nvcc` must also be installed and on PATH.
If port 8000 is occupied, stop the other service before starting this server.
If CUDA runs out of memory, check `nvidia-smi` and free GPU memory; the script
does not automatically reduce context or switch to CPU. Quantized accuracy
still needs evaluation on your tasks; passing the smoke test establishes API
integration, not routing quality.

### Smaller GLiFormer alternative

[jeff](https://github.com/logan-markewich/jeff) is a self-hosted
implementation of that API using GLiFormer. Follow its upstream installation
and model-download instructions, start it, then point forkdeck at it:

```
JEFF_API_KEYS=devkey uv run jeff        # listens on :8000
```

```toml
[router]
kind = "jev"

[router.jev]
base_url = "http://127.0.0.1:8000"
api_key_env = "FORKDECK_JEV_KEY"        # export FORKDECK_JEV_KEY=devkey
```

`forkdeck route "some task"` prints the decision as JSON, which is the quickest
way to see how a model answers. The two descriptions the model chooses between
(`claude_is_for`, `codex_is_for`) are in the config; the defaults are a
starting point, not a finding about either tool.
