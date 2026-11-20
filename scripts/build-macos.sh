#!/usr/bin/env bash
# Builds the grauwolf CLI (and, with --tools, grauwolf-tools, ADR-0118; with --k8-ops, grauwolf-k8-ops, ADR-0120) on macOS.
#
#   scripts/build-macos.sh                 release build for this Mac → dist/grauwolf
#   scripts/build-macos.sh --tools         also build the lean tool program → dist/grauwolf-tools
#   scripts/build-macos.sh --k8-ops        also build the Kubernetes operator program → dist/grauwolf-k8-ops
#   scripts/build-macos.sh --universal     one binary for Apple Silicon and Intel (lipo)
#   scripts/build-macos.sh --debug         debug build (faster to compile)
#   scripts/build-macos.sh --install       also copy the binary to ~/.local/bin
#   scripts/build-macos.sh --install DIR   … or to DIR
#
# Needs the Xcode command line tools (C compiler, make, perl – libgit2 and OpenSSL are compiled from
# their vendored sources) and rustup. The Rust version defaults to the one the images use; override it
# with RUST_TOOLCHAIN=… . The target wasm32-unknown-unknown is installed too: grauwolf builds the tool
# code of specs with it at run time.
set -euo pipefail

TOOLCHAIN="${RUST_TOOLCHAIN:-1.99}"
PROFILE="release"
UNIVERSAL=0
TOOLS=0
K8_OPS=0
INSTALL_DIR=""

usage() { sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
  case "$1" in
    --universal) UNIVERSAL=1 ;;
    --debug) PROFILE="debug" ;;
    --tools) TOOLS=1 ;;
    --k8-ops) K8_OPS=1 ;;
    --install)
      if [ $# -gt 1 ] && [ "${2#--}" = "$2" ]; then INSTALL_DIR="$2"; shift; else INSTALL_DIR="$HOME/.local/bin"; fi ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || fail "this script is for macOS (uname: $(uname -s))"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
[ -f Cargo.toml ] && [ -d crates/apps/grauwolf-cli ] || fail "run it from a grauwolf checkout ($ROOT)"

# Xcode command line tools: cc, make, perl.
if ! xcode-select -p >/dev/null 2>&1; then
  fail "the Xcode command line tools are missing – run: xcode-select --install"
fi
for tool in cc make perl; do
  command -v "$tool" >/dev/null 2>&1 || fail "'$tool' not found – install the Xcode command line tools"
done

# rustup and the toolchain.
if ! command -v rustup >/dev/null 2>&1; then
  if [ -x "$HOME/.cargo/bin/rustup" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
  else
    printf 'rustup is not installed. Install it now from https://rustup.rs? [y/N] '
    read -r answer
    case "$answer" in
      y|Y|yes) curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
               export PATH="$HOME/.cargo/bin:$PATH" ;;
      *) fail "rustup is required – see https://rustup.rs" ;;
    esac
  fi
fi

HOST_TARGET="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p' || true)"
case "$(uname -m)" in
  arm64) HOST_TARGET="aarch64-apple-darwin" ;;
  x86_64) HOST_TARGET="x86_64-apple-darwin" ;;
esac

if [ "$UNIVERSAL" = 1 ]; then
  TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
else
  TARGETS=("$HOST_TARGET")
fi

say "Rust $TOOLCHAIN with targets ${TARGETS[*]} wasm32-unknown-unknown"
rustup toolchain install "$TOOLCHAIN" --profile minimal >/dev/null
rustup target add --toolchain "$TOOLCHAIN" wasm32-unknown-unknown "${TARGETS[@]}" >/dev/null

CARGO_FLAGS=(--locked -p grauwolf-cli)
PROGRAMS=(grauwolf)
if [ "$TOOLS" = 1 ]; then
  CARGO_FLAGS+=(-p grauwolf-tools-app)
  PROGRAMS+=(grauwolf-tools)
fi
if [ "$K8_OPS" = 1 ]; then
  CARGO_FLAGS+=(-p grauwolf-k8-ops-app)
  PROGRAMS+=(grauwolf-k8-ops)
fi
[ "$PROFILE" = "release" ] && CARGO_FLAGS+=(--release)

mkdir -p dist
for target in "${TARGETS[@]}"; do
  say "Building ${PROGRAMS[*]} ($PROFILE) for $target"
  cargo "+$TOOLCHAIN" build "${CARGO_FLAGS[@]}" --target "$target"
done

for program in "${PROGRAMS[@]}"; do
  BUILT=()
  for target in "${TARGETS[@]}"; do
    BUILT+=("target/$target/$PROFILE/$program")
  done
  if [ "$UNIVERSAL" = 1 ]; then
    say "Combining both architectures of $program into one binary"
    lipo -create -output "dist/$program" "${BUILT[@]}"
  else
    cp "${BUILT[0]}" "dist/$program"
  fi
  chmod +x "dist/$program"
  say "Built $("dist/$program" --version) → $ROOT/dist/$program"
  file "dist/$program" | sed 's/^/    /'
done

if [ -n "$INSTALL_DIR" ]; then
  mkdir -p "$INSTALL_DIR"
  for program in "${PROGRAMS[@]}"; do
    cp "dist/$program" "$INSTALL_DIR/$program"
    say "Installed to $INSTALL_DIR/$program"
  done
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo "    $INSTALL_DIR is not in your PATH – add: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
  esac
fi
