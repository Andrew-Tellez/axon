#!/bin/sh
# axon installer.  curl -fsSL https://axon.andrewtellez.dev/install.sh | sh
# o: curl -fsSL https://raw.githubusercontent.com/Andrew-Tellez/axon/main/install.sh | sh
set -eu

REPO="Andrew-Tellez/axon"
BIN="axon"
DIR="${AXON_INSTALL_DIR:-$HOME/.local/bin}"
VER="${AXON_VERSION:-latest}"

case "$(uname -s)" in
  Darwin) OS="apple-darwin" ;;
  Linux)  OS="unknown-linux-gnu" ;;
  *) echo "axon: sistema no soportado: $(uname -s). Compila con: cargo install --git https://github.com/$REPO" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64|aarch64) ARCH="aarch64" ;;
  x86_64|amd64)  ARCH="x86_64" ;;
  *) echo "axon: arquitectura no soportada: $(uname -m)" >&2; exit 1 ;;
esac

TARGET="${ARCH}-${OS}"
if [ "$VER" = "latest" ]; then
  URL="https://github.com/$REPO/releases/latest/download/${BIN}-${TARGET}.tar.gz"
else
  URL="https://github.com/$REPO/releases/download/${VER}/${BIN}-${TARGET}.tar.gz"
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

echo "axon: descargando ${TARGET}..."
if ! curl -fsSL "$URL" -o "$TMP/axon.tar.gz"; then
  echo "axon: no se pudo descargar $URL" >&2
  echo "      alternativa: cargo install --git https://github.com/$REPO" >&2
  exit 1
fi
tar -xzf "$TMP/axon.tar.gz" -C "$TMP"

mkdir -p "$DIR"
install -m 755 "$TMP/$BIN" "$DIR/$BIN"
echo "axon: instalado en $DIR/$BIN"

case ":$PATH:" in
  *":$DIR:"*) "$DIR/$BIN" --version ;;
  *) echo "axon: agrega $DIR a tu PATH:  export PATH=\"$DIR:\$PATH\"" ;;
esac
