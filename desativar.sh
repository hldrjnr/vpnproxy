#!/usr/bin/env bash
# Desativa o proxy no Linux (desktop + variáveis de ambiente da sessão).
set -euo pipefail

XSESSION="$HOME/.xsessionrc"
BEGIN="# >>> proxy-toggle (gerado pela app; relogue para aplicar) >>>"
END="# <<< proxy-toggle <<<"

echo "=========================================="
echo "  DESATIVANDO PROXY"
echo "=========================================="

# 1 - Proxy do desktop
if command -v gsettings >/dev/null 2>&1; then
  gsettings set org.gnome.system.proxy mode 'none'
  echo "[1/3] gsettings: OK (none)"
else
  echo "[1/3] gsettings não encontrado." >&2
fi

# 2 - Remove o bloco de variáveis do ~/.xsessionrc
if [ -f "$XSESSION" ]; then
  awk -v b="$BEGIN" -v e="$END" '
    $0==b { inblock=1 }
    inblock { if ($0==e) inblock=0; next }
    { print }
  ' "$XSESSION" > "${XSESSION}.tmp"
  mv "${XSESSION}.tmp" "$XSESSION"
fi
echo "[2/3] ~/.xsessionrc: bloco de proxy removido"

# 3 - Cache DNS
if command -v resolvectl >/dev/null 2>&1; then
  resolvectl flush-caches 2>/dev/null || true
  echo "[3/3] cache DNS limpo"
else
  echo "[3/3] resolvectl ausente — pulando cache DNS"
fi

echo ""
echo "PROXY DESATIVADO. Reabra os programas (ou relogue) para a mudança valer."
