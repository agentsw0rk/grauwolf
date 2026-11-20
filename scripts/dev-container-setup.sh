#!/usr/bin/env bash
# Sets up the development environment in a fresh container (Debian/Ubuntu):
#   - build-essential and pkg-config (C toolchain for crates with native code; needs sudo)
#   - Rust (rustup) with toolchain 1.99, rustfmt, clippy and the wasm32-unknown-unknown target
#   - Node.js (>= 22) for studio/web (skipped when the tree has no studio/web): reported, installed via nvm when missing and ~/.nvm exists
#   - Linux native bindings for the studio's frontend tests (rolldown, tailwind oxide, lightningcss)
#     in ~/.cache/studio-linux-bindings, because studio/web/node_modules belongs to the host (never touched)
#   - ~/.grauwolf-env (PATH, CARGO_*, NAPI_RS_NATIVE_LIBRARY_PATH), sourced from ~/.zshrc and ~/.bashrc
# Idempotent: steps that are already done are skipped. Unreachable hosts skip only their step.
#
#   scripts/dev-container-setup.sh            # everything
#   scripts/dev-container-setup.sh --no-apt   # skip the C toolchain (when you cannot use sudo)
#   scripts/dev-container-setup.sh --no-rust  # skip Rust
#   scripts/dev-container-setup.sh --no-node  # skip Node.js and the frontend bindings
#   scripts/dev-container-setup.sh --check    # only report present/missing, install nothing; exit 1 if missing
set -euo pipefail

TOOLCHAIN=${GRAUWOLF_TOOLCHAIN:-1.99}
NODE_MIN=22
BINDINGS_DIR="$HOME/.cache/studio-linux-bindings"
ENV_FILE="$HOME/.grauwolf-env"
RC_MARKER="# grauwolf dev environment"
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
LOCK="$ROOT/studio/web/package-lock.json"

apt=true rust=true node=true check=false
for arg in "$@"; do
  case "$arg" in
    --no-apt) apt=false ;;
    --no-rust) rust=false ;;
    --no-node) node=false ;;
    --check) check=true ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

# The public export has no studio/web: Node.js and the frontend bindings are then not needed.
NODE_SKIP_WHY="--no-node"
if [ ! -d "$ROOT/studio/web" ] && $node; then
  node=false
  NODE_SKIP_WHY="studio/web is not part of this tree (the open source export), nothing needs Node.js"
fi

case "$(uname -m)" in
  aarch64|arm64) ARCH=arm64 ;;
  x86_64|amd64) ARCH=x64 ;;
  *) ARCH=unknown ;;
esac
NAPI_LIB="$BINDINGS_DIR/node_modules/@rolldown/binding-linux-$ARCH-gnu/rolldown-binding.linux-$ARCH-gnu.node"

# Summary: one status per component, "ok" / "missing: why" / "skipped: why".
S_C="" S_RUST="" S_NODE="" S_BIND="" S_ENV=""
required_missing=false
note_missing() { required_missing=true; }

reachable() { curl --connect-timeout 10 --max-time 20 -sI "https://$1" >/dev/null 2>&1; }

export PATH="$HOME/.cargo/bin:$PATH"

# 1. C toolchain
if $apt || $check; then
  if command -v cc >/dev/null 2>&1 && command -v pkg-config >/dev/null 2>&1; then
    S_C="ok ($(cc --version | head -1))"
  elif $check || ! $apt; then
    S_C="missing: install with sudo apt-get install -y build-essential pkg-config"; note_missing
  elif ! sudo -n true 2>/dev/null && [ ! -t 0 ]; then
    echo "C toolchain: missing, and sudo needs a password but there is no terminal here." >&2
    echo "  Run this script in a terminal of the container, or as root from the host:" >&2
    echo "  docker exec -u root <container> sh -c 'apt-get update && apt-get install -y build-essential pkg-config'" >&2
    S_C="missing: sudo needs a terminal (see above)"; note_missing
  else
    echo "C toolchain: installing build-essential and pkg-config (sudo)"
    if sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq build-essential pkg-config; then
      S_C="ok ($(cc --version | head -1))"
    else
      S_C="missing: apt-get failed (network?)"; note_missing
    fi
  fi
else
  S_C="skipped: --no-apt"
fi

# 2. Rust
rust_ready() {
  command -v rustup >/dev/null 2>&1 \
    && rustup run "$TOOLCHAIN" rustc --version >/dev/null 2>&1 \
    && rustup run "$TOOLCHAIN" cargo fmt --version >/dev/null 2>&1 \
    && rustup run "$TOOLCHAIN" cargo clippy --version >/dev/null 2>&1 \
    && rustup target list --installed --toolchain "$TOOLCHAIN" 2>/dev/null | grep -qx wasm32-unknown-unknown
}
rust_state() { echo "ok ($(rustup run "$TOOLCHAIN" rustc --version), targets: $(rustup target list --installed --toolchain "$TOOLCHAIN" | tr '\n' ' '))"; }
if ! $rust; then
  S_RUST="skipped: --no-rust"
elif rust_ready; then
  S_RUST=$(rust_state)
elif $check; then
  S_RUST="missing: Rust $TOOLCHAIN with rustfmt, clippy and wasm32-unknown-unknown"; note_missing
else
  host=static.rust-lang.org
  command -v rustup >/dev/null 2>&1 || host=sh.rustup.rs
  if ! reachable "$host"; then
    echo "Rust: $host is unreachable (no network?) - skipping the Rust install; run again when online." >&2
    S_RUST="missing: $host unreachable, install skipped"; note_missing
  else
    if ! command -v rustup >/dev/null 2>&1; then
      echo "Rust: installing rustup with toolchain $TOOLCHAIN"
      installer=$(mktemp)
      curl --proto '=https' --tlsv1.2 -sSf --connect-timeout 10 --retry 3 --retry-delay 2 https://sh.rustup.rs -o "$installer" \
        && sh "$installer" -y -q --default-toolchain "$TOOLCHAIN" --profile minimal -c rustfmt,clippy || true
      rm -f "$installer"
    else
      echo "Rust: rustup present, installing toolchain $TOOLCHAIN"
      rustup toolchain install "$TOOLCHAIN" --profile minimal -c rustfmt,clippy >/dev/null || true
      rustup default "$TOOLCHAIN" >/dev/null || true
    fi
    if command -v rustup >/dev/null 2>&1; then rustup target add --toolchain "$TOOLCHAIN" wasm32-unknown-unknown >/dev/null || true; fi
    if rust_ready; then S_RUST=$(rust_state); else S_RUST="missing: install failed (see messages above)"; note_missing; fi
  fi
fi

# 3. Node.js (studio/web)
NODE_BIN_DIR=""
node_major() { node -v 2>/dev/null | sed -E 's/^v([0-9]+).*/\1/'; }
if $node; then
  if ! command -v node >/dev/null 2>&1 && [ -s "$HOME/.nvm/nvm.sh" ]; then
    # nvm uses unset variables; load it without nounset. It also finds an installed node.
    set +u; . "$HOME/.nvm/nvm.sh"; nvm use default >/dev/null 2>&1 || nvm use node >/dev/null 2>&1 || true; set -u
  fi
  if command -v node >/dev/null 2>&1 && [ "$(node_major)" -ge "$NODE_MIN" ] 2>/dev/null; then
    S_NODE="ok (node $(node -v), npm $(npm -v 2>/dev/null || echo '?'))"
  elif command -v node >/dev/null 2>&1; then
    S_NODE="missing: node $(node -v) is older than $NODE_MIN (studio/web needs $NODE_MIN or newer)"; note_missing
  elif $check; then
    S_NODE="missing: node $NODE_MIN or newer (install nvm from https://github.com/nvm-sh/nvm, then: nvm install --lts)"; note_missing
  elif [ -s "$HOME/.nvm/nvm.sh" ]; then
    if reachable nodejs.org; then
      echo "Node: installing the LTS release via nvm"
      set +u; . "$HOME/.nvm/nvm.sh"; nvm install --lts || true; set -u
    else
      echo "Node: nodejs.org is unreachable - skipping the nvm install." >&2
    fi
    if command -v node >/dev/null 2>&1; then S_NODE="ok (node $(node -v))"; else S_NODE="missing: nvm install --lts failed or no network"; note_missing; fi
  else
    echo "Node: not found. Install nvm (https://github.com/nvm-sh/nvm), then run: nvm install --lts" >&2
    S_NODE="missing: install node $NODE_MIN or newer (nvm: nvm install --lts)"; note_missing
  fi
  if command -v node >/dev/null 2>&1; then NODE_BIN_DIR=$(dirname "$(command -v node)"); fi
else
  S_NODE="skipped: $NODE_SKIP_WHY"
fi

# 4. Linux native bindings for the frontend tests (cache outside the mounted workspace)
# Versions come from studio/web/package-lock.json: the linux-<arch>-gnu package entry if the lock
# records it, else the version of the parent package.
lock_version() { # <lock key>
  if command -v node >/dev/null 2>&1; then
    node -e 'const l=require(process.argv[1]).packages||{};const e=l["node_modules/"+process.argv[2]];if(e&&e.version)console.log(e.version)' "$LOCK" "$1" 2>/dev/null || true
  elif command -v jq >/dev/null 2>&1; then
    jq -r --arg k "node_modules/$1" '.packages[$k].version // empty' "$LOCK"
  fi
}
installed_version() { # <package>
  sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$BINDINGS_DIR/node_modules/$1/package.json" 2>/dev/null | head -1
}
if ! $node; then
  S_BIND="skipped: $NODE_SKIP_WHY"
elif [ "$ARCH" = unknown ]; then
  S_BIND="missing: unsupported architecture $(uname -m)"; note_missing
elif [ ! -f "$LOCK" ]; then
  S_BIND="missing: $LOCK not found"; note_missing
elif ! command -v node >/dev/null 2>&1; then
  S_BIND="skipped: node is missing"
else
  PKGS=() VERS=()
  for pair in "@rolldown/binding-linux-$ARCH-gnu:rolldown" "@tailwindcss/oxide-linux-$ARCH-gnu:@tailwindcss/oxide" "lightningcss-linux-$ARCH-gnu:lightningcss"; do
    pkg=${pair%%:*} parent=${pair#*:}
    ver=$(lock_version "$pkg"); [ -n "$ver" ] || ver=$(lock_version "$parent")
    if [ -z "$ver" ]; then echo "Bindings: no version of $parent in $LOCK" >&2; continue; fi
    PKGS+=("$pkg"); VERS+=("$ver")
  done
  stale=()
  for i in "${!PKGS[@]}"; do
    [ "$(installed_version "${PKGS[$i]}")" = "${VERS[$i]}" ] || stale+=("${PKGS[$i]}@${VERS[$i]}")
  done
  if [ "${#PKGS[@]}" -ne 3 ]; then
    S_BIND="missing: could not read all versions from the lock file"; note_missing
  elif [ "${#stale[@]}" -eq 0 ] && [ -f "$NAPI_LIB" ]; then
    S_BIND="ok (rolldown ${VERS[0]}, oxide ${VERS[1]}, lightningcss ${VERS[2]}; $ARCH)"
  elif $check; then
    S_BIND="missing: ${stale[*]:-$NAPI_LIB} not in $BINDINGS_DIR"; note_missing
  elif ! reachable registry.npmjs.org; then
    echo "Bindings: registry.npmjs.org is unreachable (no network?) - skipping the frontend bindings; run again when online." >&2
    S_BIND="missing: registry.npmjs.org unreachable, install skipped"; note_missing
  else
    echo "Bindings: installing ${stale[*]:-all} into $BINDINGS_DIR"
    mkdir -p "$BINDINGS_DIR"
    {
      echo '{ "name": "studio-linux-bindings", "version": "1.0.0", "private": true, "dependencies": {'
      for i in "${!PKGS[@]}"; do
        sep=","; [ "$i" -eq $((${#PKGS[@]} - 1)) ] && sep=""
        echo "  \"${PKGS[$i]}\": \"${VERS[$i]}\"$sep"
      done
      echo '} }'
    } > "$BINDINGS_DIR/package.json"
    if (cd "$BINDINGS_DIR" && npm install --no-audit --no-fund --loglevel=error) && [ -f "$NAPI_LIB" ]; then
      S_BIND="ok (installed ${stale[*]:-all})"
    else
      S_BIND="missing: npm install failed (see messages above)"; note_missing
    fi
  fi
fi

# 5. Environment file and shell rc hooks
env_content() {
  echo "# Written by scripts/dev-container-setup.sh - overwritten on every run."
  echo "export PATH=\"\$HOME/.cargo/bin:${NODE_BIN_DIR:+$NODE_BIN_DIR:}\$PATH\""
  echo 'export CARGO_TARGET_DIR="$HOME/grauwolf-target"'
  echo 'export CARGO_INCREMENTAL=0'
  echo 'export CARGO_PROFILE_DEV_DEBUG=0'
  echo 'export CARGO_PROFILE_TEST_DEBUG=0'
  echo '# 7 GB RAM: more parallel jobs got the linker OOM-killed'
  echo 'export CARGO_BUILD_JOBS=2'
  if [ "$ARCH" != unknown ]; then echo "export NAPI_RS_NATIVE_LIBRARY_PATH=\"$NAPI_LIB\""; fi
}
if $check; then
  if [ -f "$ENV_FILE" ]; then S_ENV="ok ($ENV_FILE)"; else S_ENV="missing: $ENV_FILE"; note_missing; fi
else
  env_content > "$ENV_FILE"
  skipped_rc=""
  for rc in "$HOME/.zshrc" "$HOME/.bashrc"; do
    if grep -qF "$RC_MARKER" "$rc" 2>/dev/null; then continue; fi
    if { [ -e "$rc" ] && [ -w "$rc" ]; } || { [ ! -e "$rc" ] && [ -w "$HOME" ]; }; then
      printf '\n%s\n[ -f "%s" ] && . "%s"\n' "$RC_MARKER" "$ENV_FILE" "$ENV_FILE" >> "$rc"
      echo "Env: added 'source $ENV_FILE' to $rc"
    else
      echo "Env: $rc is not writable - add this line yourself: . $ENV_FILE" >&2
      skipped_rc="$skipped_rc $(basename "$rc")"
    fi
  done
  S_ENV="ok ($ENV_FILE${skipped_rc:+; not hooked into:$skipped_rc})"
fi

echo
echo "summary:"
printf '  %-14s %s\n' "C toolchain" "$S_C" "Rust + targets" "$S_RUST" "node" "$S_NODE" "bindings" "$S_BIND" "env file" "$S_ENV"
echo
if $required_missing; then
  if $check; then echo "something is missing - run scripts/dev-container-setup.sh"; else echo "some steps could not be completed - see above and run again"; fi
  exit 1
fi
echo "ready. For builds in the current shell: source $ENV_FILE"
