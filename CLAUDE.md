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
  Exceção: envios só de documentação (`*.md`, `docs/`, modelos de issue) não
  rodam o CI nem geram versão (`paths-ignore` no `build.yml`).
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
  (só no backend de mentira), `ISLANDBAR_DEBUG_FPS_DENIED`,
  `ISLANDBAR_DEBUG_BATTERY` (`1` anota todos os periféricos/dongles encontrados
  no stderr; `fake` e `critical` simulam periféricos sem fio no teste),
  `ISLANDBAR_DEBUG_WEATHER=fake` (previsão fixa, sem internet: a rede do
  ambiente do Claude bloqueia o Open-Meteo e o BigDataCloud; o teste no
  Windows do GitHub usa a previsão de verdade).

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
- Pedidos do iFood: a extensão (`extension/`) vai dentro do .exe (`include_str!`
  em `src/delivery.rs`) e manda o pedido para `127.0.0.1:47615`. Dá para testar
  a cadeia inteira aqui no Linux: ilha sob Xvfb + Playwright/Chromium com
  `--load-extension` e uma página falsa do iFood via `context.route` (servir o
  HTML com `charset=utf-8`, senão "Previsão" vira lixo e não é lida).
  Com `context.route` a API `chrome.cookies` da extensão não vê os cookies;
  para testar o segundo plano (cookies, renovação), usar um servidor HTTPS
  local de verdade na porta 443 e o Chromium com `--host-resolver-rules=MAP
  www.ifood.com.br 127.0.0.1,MAP marketplace.ifood.com.br 127.0.0.1`,
  `--no-proxy-server` e `--ignore-certificate-errors`; o `poll()` do
  service worker pode ser chamado direto com `sw.evaluate`.

- Clima (`src/weather.rs`): pedir a localização do Windows
  (`Geolocator::RequestAccessAsync`) com ela desligada abre uma janela do
  sistema que escurece a tela inteira e trava os cliques até alguém responder.
  Por isso a ilha só usa a localização se ela já estiver ligada (lê o
  `ConsentStore\location` no registro) e só pede pelo botão "Usar localização
  do Windows" da página Clima. Sem ela, a cidade vem pela internet (no GitHub
  aparece uma cidade dos EUA, é a máquina deles).

## Pendências (lembrar o dono)

Ao começar uma conversa nova, lembrar o dono destas tarefas em aberto:

- **Pedidos do iFood: testar num pedido de verdade** (extensão 1.4 ou mais
  nova). Ele vai conferir a pílula nova, o Ctrl em cima dela, a janela do
  pedido com o mapa e se ela aparece **sem nenhuma aba do iFood aberta**
  (só o navegador rodando). Antes, recarregar a extensão (↻ em
  `chrome://extensions`) e entrar uma vez no site do iFood.
- **Pedir o diagnóstico da extensão** (ícone da extensão → "Copiar
  diagnóstico", com a aba do pedido na frente). Com ele, ajustar `page.js` ao
  site real: se o site informa a posição do entregador (mapa), se a previsão
  de entrega é lida e se o contador de mensagens separa as da loja das dele.
  No diagnóstico da página inicial: `login` e `emAndamento.deuCerto` dizem se a
  consulta de pedidos em andamento (`marketplace.ifood.com.br/v4/customers/me/
  orders?status=ONGOING`, com os cabeçalhos copiados das consultas do site)
  funcionou; `respostas` mostra o formato da lista. Até agora só foi testado
  com páginas falsas do iFood.
- **Acompanhar sem aba aberta: feito na extensão 1.4, conferir no uso real.**
  O que o diagnóstico real (extensão 1.3) mostrou: o crachá é um JWT de 2 h no
  cookie `aAccessToken` (www.ifood.com.br, sem HttpOnly); a renovação fica no
  cookie `aRefreshToken` e vai por `POST /site-api/v2/access_tokens` com
  `{refresh_token}`, que devolve `{access_token, refresh_token}`. Só cookies
  não bastam (401), mas a extensão com o crachá consegue a lista (200). A
  lista `ONGOING` ignora o filtro e traz os últimos pedidos (com `lastStatus`,
  previsão, código, entregador e coordenadas). O `background.js` lê os
  cookies, renova e grava de volta (o site continua logado). Conferir no
  diagnóstico (`segundoPlano`): `cookies.acesso/renovar` true, `renovou` > 0
  depois de 2 h, `gravouCookie` true, e se o site continua logado depois.
  Risco: o iFood usa PerimeterX (cabeçalho `x-px-cookies`); se começar a
  recusar (403), o segundo plano espera 10 min. Ritmo (extensão 1.5): lista
  a cada minuto (só sem aba; com aba, a aba procura), e com pedido em
  andamento o service worker consulta as etapas a cada 15 s e os detalhes a
  cada 30 s, com ou sem aba (aba de fundo é freada pelo navegador a 1/min).
  Se o iFood recusar por excesso, aumentar `FAST_MS` em `background.js`.
- **Depois disso, publicar a extensão na loja**, para instalar com um clique
  e atualizar sozinha (sem "Modo do desenvolvedor"):
  - Loja do Edge (Microsoft Partner Center): gratuita;
  - Chrome Web Store: taxa única de 5 dólares.
  As contas de desenvolvedor ficam no nome dele. O Claude prepara o pacote
  (.zip da pasta `extension/`, ícones, textos, política de privacidade) e o
  passo a passo. Não fazer antes do teste real e do diagnóstico.
