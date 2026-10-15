#!/usr/bin/env bash
# Gera o app em modo release e instala só para o seu usuário (sem sudo):
#   ~/.local/bin/pauta, ícone, entrada no menu de aplicativos.
# Uso: scripts/instalar-linux.sh [--autostart]   (--autostart abre o app ao entrar na sessão)
#      scripts/instalar-linux.sh --remover
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="$HOME/.local/bin"
APPS="$HOME/.local/share/applications"
ICONES="$HOME/.local/share/icons/hicolor/512x512/apps"
AUTO="$HOME/.config/autostart"

if [[ "${1:-}" == "--remover" ]]; then
  pkill -x pauta 2>/dev/null || true
  rm -f "$BIN/pauta" "$APPS/pauta.desktop" "$ICONES/pauta.png" "$AUTO/pauta.desktop"
  echo "Removido. Seus dados continuam em ~/.local/share/app.busca.overlay (apague à mão se quiser)."
  exit 0
fi

npm install
npm run build -- --no-bundle   # só o executável, sem .deb/.rpm

mkdir -p "$BIN" "$APPS" "$ICONES"
pkill -x pauta 2>/dev/null || true
sleep 1
install -m 755 target/release/pauta "$BIN/pauta"
install -m 644 src-tauri/icons/icon.png "$ICONES/pauta.png"

cat > "$APPS/pauta.desktop" <<DESK
[Desktop Entry]
Type=Application
Name=Pauta
Comment=Busca de conversas e transcrição de reuniões
Exec=$BIN/pauta
Icon=pauta
Categories=Office;Utility;
Terminal=false
DESK

if [[ "${1:-}" == "--autostart" ]]; then
  mkdir -p "$AUTO"
  cp "$APPS/pauta.desktop" "$AUTO/pauta.desktop"
  echo "Vai abrir sozinho ao entrar na sessão."
fi

command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" || true
echo "Instalado em $BIN/pauta. Procure \"Pauta\" no menu de aplicativos."
echo "Se 'pauta' não for achado no terminal, ponha $BIN no PATH."
