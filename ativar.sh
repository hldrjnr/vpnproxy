#!/usr/bin/env bash
# Ativa o proxy no Linux (desktop + variáveis de ambiente da sessão).
# Uso: ./ativar.sh [ip] [porta] [exceções]
#   padrão: ./ativar.sh 10.71.145.245 8080 "localhost;127.0.0.1;*.local"
# As variáveis valem para os programas abertos a partir do próximo login.
set -euo pipefail

HOST="${1:-10.71.145.245}"
PORT="${2:-8080}"
BYPASS="${3:-localhost;127.0.0.1;*.local;ipc.localhost;tauri.localhost;asset.localhost}"
URL="http://${HOST}:${PORT}"
NO_PROXY="${BYPASS//;/,}"
XSESSION="$HOME/.xsessionrc"
BEGIN="# >>> proxy-toggle (gerado pela app; relogue para aplicar) >>>"
END="# <<< proxy-toggle <<<"

echo "=========================================="
echo "  ATIVANDO PROXY — ${URL}"
echo "=========================================="

# 1 - Proxy do desktop (XFCE/Cinnamon/MATE/GNOME leem org.gnome.system.proxy)
if command -v gsettings >/dev/null 2>&1; then
  IGNORE="["
  first=1
  IFS=';' read -ra ITEMS <<< "$BYPASS"
  for item in "${ITEMS[@]}"; do
    item="${item//[[:space:]]/}"
    [ -z "$item" ] && continue
    if [ "$first" -eq 1 ]; then first=0; else IGNORE+=", "; fi
    IGNORE+="'$item'"
  done
  IGNORE+="]"
  gsettings set org.gnome.system.proxy mode 'manual'
  gsettings set org.gnome.system.proxy ignore-hosts "$IGNORE"
  gsettings set org.gnome.system.proxy.http host "$HOST"
  gsettings set org.gnome.system.proxy.http port "$PORT"
  gsettings set org.gnome.system.proxy.https host "$HOST"
  gsettings set org.gnome.system.proxy.https port "$PORT"
  echo "[1/3] gsettings: OK (manual, ${HOST}:${PORT})"
else
  echo "[1/3] gsettings não encontrado — proxy do desktop não configurado." >&2
fi

# 2 - Variáveis persistentes da sessão (~/.xsessionrc é lido no login)
touch "$XSESSION"
awk -v b="$BEGIN" -v e="$END" '
  $0==b { inblock=1 }
  inblock { if ($0==e) inblock=0; next }
  { print }
' "$XSESSION" > "${XSESSION}.tmp"
{
  printf '%s\n' "$BEGIN"
  printf 'export HTTP_PROXY="%s"\nexport HTTPS_PROXY="%s"\nexport ALL_PROXY="%s"\n' "$URL" "$URL" "$URL"
  printf 'export NO_PROXY="%s"\n' "$NO_PROXY"
  printf 'export http_proxy="%s"\nexport https_proxy="%s"\nexport all_proxy="%s"\n' "$URL" "$URL" "$URL"
  printf 'export no_proxy="%s"\n' "$NO_PROXY"
  printf '%s\n' "$END"
} >> "${XSESSION}.tmp"
mv "${XSESSION}.tmp" "$XSESSION"
echo "[2/3] ~/.xsessionrc: variáveis gravadas (valem após relogar)"

# 3 - Cache DNS
if command -v resolvectl >/dev/null 2>&1; then
  resolvectl flush-caches 2>/dev/null || true
  echo "[3/3] cache DNS limpo"
else
  echo "[3/3] resolvectl ausente — pulando cache DNS"
fi

echo ""
echo "PROXY ATIVADO: ${URL}"
echo "Importante: relogue (saia e entre da sessão) ou reabra os programas."
