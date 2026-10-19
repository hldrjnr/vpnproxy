# Proxy Toggle (Tauri v2)

Utilitário de desktop (~10 MB) que liga/desliga o proxy do Windows em todo o
PC — o mesmo que `Ativar.bat` / `desativar.bat` faziam:

- `netsh winhttp` (serviços do Windows — exige **admin**)
- WinINET via registro `HKCU\...\Internet Settings` (navegadores e apps)
- variáveis persistentes `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY`
- `ipconfig /flushdns`

Janela compacta (392×532, altura ajustada ao conteúdo): botão power,
campos IP/porta, rede ao vivo (↓/↑, gráfico, latência e IP via proxy).
Minimizar oculta para a bandeja do sistema — de lá dá para
reabrir, ligar/desligar e sair. Fechar (X) encerra a app. Sem logs, sem excesso.

## Pré-requisitos (Windows)

- Node 20+ e pnpm 9/10
- Rust estável MSVC + **VS 2022 Build Tools com workload C++** (traz MSVC e
  Windows SDK) + WebView2 — o setup está sendo instalado; ver "Build" abaixo

## Instalar e rodar (neste diretório)

```powershell
pnpm install --ignore-workspace
pnpm tauri dev
```

> O `--ignore-workspace` é porque esta pasta vive dentro de um workspace pnpm
> maior (`apps/*`); ele mantém o `node_modules` local.

## Build do instalador

```powershell
pnpm tauri build
```

Gera o instalador NSIS em `src-tauri\target\release\bundle\nsis\`.
Na primeira vez o Tauri baixa sozinho o NSIS e o WiX.

## Notas

- A app se eleva sozinha ao abrir (prompt UAC, como os `.bat` exigiam admin):
  sempre roda elevada e nenhum terminal abre em momento algum.
- Depois de ligar/desligar, feche e reabra os programas.
- Programas com proxy próprio ignoram a configuração do sistema.
