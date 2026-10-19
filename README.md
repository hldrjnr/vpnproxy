# Proxy Toggle (vpnproxy) — Tauri v2

Utilitário de desktop (~10 MB) que liga/desliga o proxy do PC inteiro com um
botão — o mesmo que os antigos `Ativar.bat` / `desativar.bat` faziam no Windows,
agora também no **Linux** (Linux Mint 22.3 / Ubuntu 24.04, x86_64).

- **Windows**: `netsh winhttp` + WinINET (registro) + variáveis persistentes (`setx`).
- **Linux**: `gsettings` (`org.gnome.system.proxy`, lido por XFCE/Cinnamon/MATE/GNOME)
  + variáveis persistentes da sessão em `~/.xsessionrc` + `resolvectl flush-caches`.

Janela compacta (392×532): botão power, campos IP/porta, rede ao vivo
(↓/↑, gráfico, latência e IP via proxy). Minimizar oculta para a bandeja do
sistema — de lá dá para reabrir, ligar/desligar e sair.

## Como funciona no Linux

| Etapa | O que faz | Precisa de root? |
| --- | --- | --- |
| `gsettings set org.gnome.system.proxy mode 'manual'` (+ http/https host/port, ignore-hosts) | Proxy do desktop: XFCE (Mint), Cinnamon, MATE e GNOME leem daqui. É o mesmo lugar que a tela "Proxy de rede" das configurações mostra. | Não |
| Bloco gerado em `~/.xsessionrc` | `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY` (maiúsculas **e** minúsculas) para a sessão inteira. O `/etc/X11/Xsession` lê este arquivo no login. | Não |
| `resolvectl flush-caches` | Limpa o cache DNS (systemd-resolved). Se faltar, é só ignorado. | Não |

A app **não precisa de admin no Linux** — a pílula "Admin" só aparece quando há
elevação (Windows). No Windows o comportamento continua o mesmo: a app se eleva
sozinha via UAC ao abrir.

> **Importante:** as variáveis de ambiente valem para programas abertos **após**
> relogar (ou reabrir o programa). O proxy do desktop (gsettings) aplica na hora
> para apps que o respeitam. Programas com proxy próprio (ex.: navegadores com
> configuração manual) ignoram a configuração do sistema.

## Baixar a versão pronta (GitHub Actions)

1. Abra a aba **Actions** → **Build Proxy Toggle Linux x64**.
2. Clique em **Run workflow** (ou use a última execução verde de um push no `main`).
3. No fim da execução, em **Artifacts**, baixe:
   - `Proxy-Toggle-Linux-x64-deb` → contém o `.deb` (recomendado)
   - `Proxy-Toggle-Linux-x64-binario` → só o binário (portátil)
4. Instale o `.deb`:

   ```bash
   sudo apt install ./Proxy*Toggle*_amd64.deb
   ```

   O pacote declara as dependências (`libwebkit2gtk-4.1-0`, `libgtk-3-0`,
   `libayatana-appindicator3-1`) — o apt resolve tudo. Depois abra o menu e
   procure **Proxy Toggle**.

   Para o binário solto (sem instalar):

   ```bash
   chmod +x Proxy-Toggle*   # o zip do artifact não guarda o bit de execução
   ./Proxy-Toggle*
   ```

## Build local (Linux Mint 22.3 / Ubuntu 24.04)

```bash
# dependências de build (uma vez)
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev

# Node 20+ e pnpm 9/10
pnpm install --frozen-lockfile
pnpm tauri dev          # rodar em modo dev

# gerar o .deb
pnpm build:linux         # equivale a: pnpm tauri build --bundles deb
```

O `.deb` sai em `src-tauri/target/release/bundle/deb/`.
O bundle de Linux é configurado em `src-tauri/tauri.linux.conf.json`
(`targets: ["deb"]`); para gerar AppImage também: `pnpm tauri build --bundles appimage`.

## Scripts de terminal (Linux)

Os equivalentes dos `.bat` para usar sem abrir a app:

```bash
./ativar.sh 10.71.145.245 8080     # liga o proxy (desktop + ~/.xsessionrc)
./desativar.sh                    # desliga tudo
```

## Windows (histórico e ainda suportado)

- `Ativar.bat` / `desativar.bat`: os scripts originais (exigem admin).
- A app Windows continua idêntica: WinHTTP + WinINET + registro `HKCU\Environment`
  + `ipconfig /flushdns`, autoelevando via UAC. Build: `pnpm tauri build`
  (gera NSIS; exige Rust MSVC + VS 2022 Build Tools + WebView2).

## Estrutura

```
.github/workflows/build-linux.yml   # CI: build do .deb no ubuntu-24.04
ativar.sh / desativar.sh            # proxy por terminal (Linux)
Ativar.bat / desativar.bat          # proxy por terminal (Windows)
src/                                # frontend (vite + @tauri-apps/api)
src-tauri/src/main.rs               # núcleo: módulo platform (windows|unix)
src-tauri/tauri.conf.json           # config base (Windows/NSIS)
src-tauri/tauri.linux.conf.json     # config Linux (deb) — mesclada automaticamente
```

## Notas

- No Linux a medição de rede usa `/proc/net/dev` (uma leitura, sem spawn);
  no Windows, `GetIfTable2` com fallback PowerShell.
- Testes: `cargo test --manifest-path src-tauri/Cargo.toml` roda em ambos os
  sistemas (testes de registro/UAC só no Windows; bloco `~/.xsessionrc`,
  parsing de `/proc/net/dev` e exceções no Linux).
- Programas com proxy próprio ignoram a configuração do sistema.
