# Islandbar: guia para o Claude

Barra estilo "Dynamic Island" para Windows, em Rust (eframe/egui 0.37 com glow,
winit, crate `windows`). Mostra mídia, mixer de volume, CPU/RAM/GPU/temperatura
e um modo jogo com FPS. Veja o README para a lista de funções e de arquivos.

## Como falar com o dono do projeto

- Responder sempre em **português do Brasil, linguagem simples**. Ele não é
  programador Rust experiente: explicar causa e solução sem jargão, e dizer o que
  ele precisa fazer no PC dele (passo a passo) quando for o caso.
- Ele testa no próprio PC (Windows, placa NVIDIA RTX 5060) e conta o resultado.
  Ao terminar uma mudança, dizer como ele confere (ex.: número da versão no
  rodapé do painel) e o que observar.
- Comentários no código e textos da interface ficam em português.
- Mensagens de commit em inglês, curtas.

## Metas que valem para qualquer mudança

- **Leve acima de tudo**: pouca CPU, pouca RAM, sem atrapalhar jogos. Parada,
  a ilha dorme (cerca de 1 quadro a cada 2 s); só redesenha quando algo muda.
  Não adicionar trabalho por quadro nem leituras frequentes do sistema sem
  necessidade. Ler só o que está aparecendo na tela.
- Antes/depois de mexer em desempenho, medir com `ISLANDBAR_DEBUG_PERF=1`
  (linha `perf:` a cada 5 s no stderr) — o teste no Windows já registra isso.
- No modo jogo a ilha fica "quieta" e não pode roubar o foco do jogo.

## Versões e atualização (cuidado)

- **Push no `main` publica uma versão** (`build-N` no GitHub Releases), e as
  ilhas instaladas se atualizam sozinhas em ~2 minutos. Só enviar ao `main`
  código compilando e testado.
- Para testar sem publicar, usar uma branch `ci/<nome>`: o GitHub Actions
  compila e roda o teste visual, mas não cria versão.
- O dono também faz commits direto no `main`: sempre `git pull --rebase`
  antes de enviar.

## Como testar

O ambiente do Claude é Linux; a ilha é para Windows.

- `cargo build` / `cargo clippy` no Linux compilam com o backend de mentira
  (`src/backend/mock.rs`), útil para checar erros de compilação.
- O CI falha se `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
  ou `cargo test` não passarem. Antes de enviar: `cargo fmt` e conferir o clippy
  também com `--target x86_64-pc-windows-gnu` (o alvo já está instalado aqui).
- O teste de verdade roda no Windows do GitHub Actions (`.github/workflows/build.yml`
  e `scripts/smoke.ps1`): abre a ilha com OpenGL por software (Mesa), simula o
  mouse, abre um jogo falso (`examples/fake_game.rs`), tira screenshots e
  registra logs.
- Os resultados vão para a branch `ci-results` (sobrescrita a cada execução):
  `git fetch origin ci-results`, conferir se o assunto do commit tem o SHA
  testado, e `git archive FETCH_HEAD | tar -x` numa pasta temporária.
  Acompanhar a execução com `gh run list` / `gh api` (o `gh run download` não
  funciona aqui).
- Variáveis de diagnóstico: `ISLANDBAR_DEBUG_PERF`, `ISLANDBAR_DEBUG_UI`,
  `ISLANDBAR_DEBUG_GAME`, `ISLANDBAR_DEBUG_SYSMON`, `ISLANDBAR_DEBUG_SILENT`
  (só no backend de mentira), `ISLANDBAR_DEBUG_FPS_DENIED`.

## Coisas já aprendidas

- Acordar a interface: usar `ctx.request_repaint_after(1ms)`; o
  `request_repaint` imediato do egui desenha 2 quadros.
- Escrever logs com `perf::line` (uma escrita só), não `eprintln!` com muitos campos.
- Ficar na frente de jogos: `window::stay_above_large_windows` em
  `src/platform.rs` só reafirma "sempre no topo" quando uma janela grande cobre
  a ilha (no máximo a cada 400 ms). Não trocar por um `SetWindowPos` a cada quadro.
- Jogos Vulkan/OpenGL em placa NVIDIA escondem a ilha mesmo sem bordas; não dá
  para resolver pelo lado da ilha. Solução: no Painel NVIDIA, "Método de
  apresentação Vulkan/OpenGL" → "Preferir em camadas na cadeia de troca DXGI"
  para o jogo (está no README).
- Na máquina do GitHub a temperatura aparece "n/d" (limitação dela, não bug).
- Barra de título antiga: aparece quando a ilha ganha/perde o foco (ex.: ao
  digitar no alarme). O `window_hook` em `src/platform.rs` repassa
  `WM_NCACTIVATE` com `lParam = -1` e tira os bits de moldura em
  `WM_STYLECHANGING` (só `GWL_STYLE`). Não bloquear `WM_NCPAINT` nem mexer no
  estilo estendido ali: forçar o `GWL_EXSTYLE` congelou a imagem da ilha (ela não
  voltava depois do Ctrl). A máquina do GitHub não mostra essa barra; o teste
  visual confere, mas a confirmação é no PC do dono.
