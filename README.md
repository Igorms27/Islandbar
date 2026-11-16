<div align="center">

# Islandbar

**Uma "Dynamic Island" para o Windows, leve e animada.**

Fica no topo da tela e se abre num painel com mixer de volume, mídia, troca de
fone/caixa de som, alarme e o uso do PC. Nos jogos, mostra FPS, CPU, GPU e RAM.
E acompanha o seu pedido do iFood numa segunda pílula, com mapa.

[![Build](https://github.com/Igorms27/Islandbar/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/Igorms27/Islandbar/actions/workflows/build.yml)
[![Versão](https://img.shields.io/github/v/release/Igorms27/Islandbar?label=vers%C3%A3o)](https://github.com/Igorms27/Islandbar/releases/latest)
[![Licença: GPL-3.0](https://img.shields.io/badge/licen%C3%A7a-GPL--3.0-blue)](LICENSE)
![Windows 10/11](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D6)
![Feito em Rust](https://img.shields.io/badge/feito%20em-Rust-orange)

**[⬇ Baixar o islandbar.exe](https://github.com/Igorms27/Islandbar/releases/latest/download/islandbar.exe)**

<img src="docs/images/pilula.png" alt="A ilha no topo da tela: à esquerda a pílula do pedido do iFood; na principal, o pet fantasma, o uso do PC, um alarme marcado e o relógio" width="560">

</div>

<p align="center">
  <img src="docs/images/painel-som.png" alt="Painel aberto na página Som: saída de som, microfone, mixer e uso do PC" width="45%">
  &nbsp;
  <img src="docs/images/painel-alarme.png" alt="Painel na página Alarme: escolha de horas e minutos" width="45%">
</p>

## Sumário

- [Recursos](#recursos)
- [Instalação](#instalação)
- [Como usar](#como-usar)
- [Pedidos do iFood](#pedidos-do-ifood)
- [Leve de verdade](#leve-de-verdade)
- [Solução de problemas](#solução-de-problemas)
- [Para desenvolvedores](#para-desenvolvedores)
- [Licença](#licença)

## Recursos

- **Pílula no topo** com relógio, música atual e barrinhas que dançam com o som.
- **Pets**: troque as barrinhas por um fantasminha ou um robozinho. Eles piscam, olham para os lados
  e abrem a boca cantando quando sai som, mesmo com o volume baixo. A escolha fica no cartão inicial. Sem música, mostra
  o uso de CPU, GPU e RAM.
- **Mixer**: volume geral e de cada aplicativo, com medidor de áudio ao vivo dentro de cada slider.
  Clique no ícone para deixar no mudo.
- **Saída de som e microfone**: um botão para cada saída (fone, TV, caixa de som) e para cada
  microfone. Um clique troca o que o Windows usa para música, jogos e chamadas.
- **Mídia**: título, artista e os botões anterior / tocar-pausar / próxima. Funciona com Spotify,
  navegadores e qualquer app que apareça no controle de mídia do Windows.
- **Alarme**: "toca daqui a X horas/minutos", com setinhas, atalhos ou digitando o número. Toca
  mesmo com a ilha fechada: na hora, o Windows abre a ilha sozinho (por uma tarefa do Agendador de
  Tarefas, "Islandbar - alarme", que some depois).
  Na hora, a ilha mostra um aviso com bip de despertador e os botões Soneca e Parar.
- **Aviso de volume**: ao usar as teclas de volume do teclado, a ilha se estica e mostra o nível.
- **Monitor do sistema**: CPU, GPU, RAM e temperatura, sem instalar driver. No cartão inicial você
  escolhe o sensor da temperatura (placa de vídeo, placa-mãe, disco ou automático) e quais números
  aparecem na ilha fechada e no modo jogo.
- **Modo jogo**: com um jogo em tela cheia, uma pílula ao lado da ilha mostra FPS, CPU, GPU e RAM.
  Navegadores e players de vídeo em tela cheia não contam como jogo.
- **Pedidos do iFood**: com o site do iFood aberto no Chrome ou no Edge (qualquer página, até a
  inicial, numa aba de fundo), uma segunda pílula à esquerda da ilha mostra a etapa (em preparo, a caminho, entregador
  chegou…), a previsão de entrega, o código de entrega e quantas mensagens o chat do pedido tem.
  Ela dá um pulinho quando algo muda. Clicando nela abre a janela do pedido: andamento, entregador
  e um mapa com a loja, a sua casa e o entregador (quando o site informa onde ele está). Precisa de
  uma extensão pequena, instalada uma vez (veja [Pedidos do iFood](#pedidos-do-ifood)).
- **Iniciar com o Windows** (opcional) e **atualização automática**.

<p align="center">
  <img src="docs/images/pets.png" alt="Os três visuais da ilha tocando música: barrinhas, fantasma e robô" width="640">
  <br><br>
  <img src="docs/images/modo-jogo.png" alt="Modo jogo: a pílula com FPS, CPU e GPU ao lado da ilha" width="480">
  <br><br>
  <img src="docs/images/alarme-tocando.png" alt="Aviso do alarme tocando, com os botões Soneca e Parar" width="400">
</p>

<p align="center">
  <img src="docs/images/pedido-ifood.png" alt="Janela do pedido do iFood: etapa, previsão de entrega, andamento, mapa com a loja, a casa e o entregador, e o código de entrega" width="45%">
  &nbsp;
  <img src="docs/images/cartao-inicial.png" alt="Cartão inicial: iniciar com o Windows, números da ilha, pet, sensor da temperatura e a extensão do iFood" width="45%">
</p>

## Instalação

Não precisa instalar nada além do próprio programa.

1. Baixe o **[islandbar.exe](https://github.com/Igorms27/Islandbar/releases/latest/download/islandbar.exe)**
   da versão mais recente.
2. Coloque o arquivo numa pasta sua (ex.: `Documentos\Islandbar`) e abra.
3. Na primeira vez, um cartão pergunta se a ilha deve abrir junto com o Windows.

> [!NOTE]
> O Windows pode mostrar o aviso "O Windows protegeu o computador" (SmartScreen), porque o
> executável não tem uma assinatura digital paga. Clique em **Mais informações → Executar assim mesmo**.

**Atualizações:** a ilha confere a cada 2 minutos se saiu uma versão nova. Se saiu, ela baixa, troca o
executável e reinicia sozinha quando estiver recolhida. Durante um jogo, ou com o alarme tocando, ela
espera. Um alarme marcado continua contando depois de reiniciar. A versão atual aparece no rodapé do painel. As notas de cada versão estão em
[Releases](https://github.com/Igorms27/Islandbar/releases).

## Como usar

| Para… | Faça |
| --- | --- |
| Abrir o painel | Clique na ilha. Ele fecha sozinho quando o mouse sai. |
| Mudar de página (Som ↔ Alarme) | Arraste o painel para o lado, use a rodinha lateral ou clique nas bolinhas do rodapé. |
| Trocar fone / TV / microfone | Página Som: clique no botão do dispositivo. |
| Marcar um alarme | Página Alarme: setinhas, atalhos (+5 min, +15 min…) ou clique no número e digite. **Enter** inicia. |
| Tirar a ilha da frente | Com o mouse em cima dela, aperte **Ctrl**. Ela some e os cliques passam para o que está atrás (dá para soltar o Ctrl e clicar normalmente). Volta quando o mouse sai. |
| Medir o FPS nos jogos | Na primeira vez, clique em **Ativar** no painel. O Windows pede confirmação de administrador uma vez, e passa a valer depois de sair e entrar na conta. |
| Fechar a ilha | Abra o painel e clique em **Sair**. Um alarme marcado continua valendo: na hora, o Windows abre a ilha e ele toca. Para desistir dele, cancele na página Alarme antes de sair. |
| Ligar/desligar o início com o Windows | Abra o `islandbar.exe` de novo: o cartão aparece outra vez. |
| Acompanhar um pedido do iFood | Deixe uma aba do iFood aberta no navegador, logado (qualquer página, pode ser de fundo). Não precisa abrir o pedido: vale até para pedidos feitos pelo celular. A pílula aparece sozinha e some uns minutos depois da entrega ou quando a aba fecha. Clique nela para ver a janela do pedido, com o mapa; o Ctrl em cima dela também tira as duas pílulas da frente. |
| Escolher o pet, os números da ilha e a temperatura | Também no cartão inicial (abra o `islandbar.exe` de novo). Cada opção de temperatura mostra o valor de agora; "n/d" quer dizer que o PC não tem aquele sensor. |

### Pedidos do iFood

A ilha não lê o navegador sozinha: quem lê o iFood é uma extensão pequena, que vai junto com a
ilha. Ela só funciona em `www.ifood.com.br`, só manda para a ilha no seu próprio PC (`127.0.0.1`)
e não guarda nem envia seu login.

Com qualquer página do iFood aberta e você logado, a extensão pergunta ao iFood, a cada minuto
ou dois, se há pedido em andamento, do mesmo jeito que o próprio site pergunta. Para isso ela
usa o login que o site já está usando naquela aba, só para falar com o iFood. Não precisa abrir o
pedido, e vale também para pedidos feitos pelo celular na mesma conta.

Para instalar (uma vez só), no Chrome ou no Edge:

1. Abra o `islandbar.exe`. No cartão inicial, clique em **Instalar extensão**. Abre a pasta
   `%LOCALAPPDATA%\Islandbar\extensao-ifood`.
2. No navegador, abra `chrome://extensions` (no Edge, `edge://extensions`).
3. Ligue o **Modo do desenvolvedor** (no Chrome fica no canto de cima; no Edge, no menu da esquerda).
4. Clique em **Carregar sem compactação** e escolha a pasta que abriu.
5. Recarregue a aba do iFood (F5).

Com a extensão instalada, o cartão inicial mostra **Instalada** no lugar do botão.

Depois disso, é só deixar uma aba do iFood aberta. Dica: clique com o botão direito na aba e
escolha **Fixar** para ela ficar pequenininha no canto. As atualizações da ilha também atualizam
os arquivos da extensão. Para o navegador usar a versão nova na hora, clique no botão de
recarregar (↻) da extensão em `chrome://extensions` e aperte F5 na aba do iFood.

O mapa da janela do pedido vem do [OpenStreetMap](https://www.openstreetmap.org/copyright)
(escurecido pela ilha) e só é baixado com a janela aberta.

Clicando no ícone da extensão (na barra do navegador) aparece se a ilha está recebendo o pedido e o
botão **Copiar diagnóstico**. Ele copia só o formato das respostas do site do iFood (nomes dos
campos, sem nomes, endereços ou números), para ajudar a ajustar a leitura quando o site muda.
Também diz onde o site guarda o login e como o renova, só pelos nomes e tempos, nunca o login em
si, e testa se a extensão consegue consultar a lista de pedidos sozinha. Esse teste é só de
leitura e não guarda nada.

## Leve de verdade

A ilha foi feita para não pesar no PC nem roubar desempenho dos jogos.

- **Parada, ela dorme**: só é redesenhada quando algo na tela muda, cerca de 1 vez por segundo,
  com uso de CPU perto de zero.
- **Com som tocando**, as barrinhas animam a 30 quadros por segundo. Abrir, fechar e o painel aberto
  usam a taxa do monitor, para ficarem lisos.
- **Com um pet**, parada ela acorda só durante os gestos (menos de 1 segundo, a cada poucos segundos).
  As barrinhas continuam sendo a opção mais leve.
- **No modo jogo ela fica quieta**: não dança com o som, o pet não faz gestos e só atualiza os números,
  1 vez por segundo.
- **Só lê o que está na tela**: o volume de cada app, a temperatura e a lista de dispositivos só são
  consultados com o painel aberto.
- **Memória**: 30 segundos depois do último uso, ela devolve ao Windows a memória que ficou parada.

Para medir no seu PC, abra pelo terminal com `ISLANDBAR_DEBUG_PERF=1` e o stderr indo para um arquivo
(`islandbar.exe 2> perf.txt`). A cada 5 segundos ela anota quadros por segundo, uso de CPU e memória.

## Solução de problemas

<details>
<summary><b>A ilha some atrás de um jogo em tela cheia</b></summary>

A ilha volta para a frente sozinha mesmo quando o jogo se marca como "sempre no topo". Duas exceções:

- **Tela cheia exclusiva**: nesse modo o Windows não mostra nenhuma outra janela. Use o modo
  "janela sem bordas" (borderless) nas opções de vídeo do jogo.
- **Jogos Vulkan/OpenGL em placa NVIDIA** (ex.: Savage Wind Recomp): mesmo sem bordas, o driver pode
  mandar a imagem direto para o monitor. Para resolver, vá no Painel de Controle NVIDIA →
  **Gerenciar as configurações 3D** → **Configurações de programa**, adicione o `.exe` do jogo e mude
  **Método de apresentação Vulkan/OpenGL** para **Preferir em camadas na cadeia de troca DXGI**
  (em inglês, *Prefer layered on DXGI Swapchain*).

</details>

<details>
<summary><b>O FPS não aparece no modo jogo</b></summary>

Falta a permissão do Windows para medir o FPS de outros programas (a mesma que a Xbox Game Bar usa).
Abra o painel, clique em **Ativar**, confirme e depois saia e entre de novo na sua conta do Windows.

</details>

<details>
<summary><b>A temperatura aparece "n/d"</b></summary>

O sensor escolhido não existe nesse PC (ou não dá para ler sem instalar um driver). Abra o
`islandbar.exe` de novo e escolha outro no cartão inicial: cada opção mostra o valor que consegue ler.

</details>

<details>
<summary><b>A pílula do iFood não aparece</b></summary>

- Confira se a extensão "Islandbar - pedidos do iFood" está ligada em `chrome://extensions` (ou
  `edge://extensions`) e recarregue a aba do iFood (F5).
- Confira se você está logado no site. Se mesmo assim não aparecer, abra a página do pedido
  (**Pedidos** → **Acompanhar**): ela sempre funciona.
- Depois de abrir a aba, a pílula pode levar até 2 minutos para aparecer.
- Se ela some depois de um tempo com a aba de fundo, a "Economia de memória" do navegador pode ter
  colocado a aba para dormir. Em Configurações → Desempenho, adicione `www.ifood.com.br` em
  "Sempre manter estes sites ativos".
- Firefox não é suportado.

</details>

<details>
<summary><b>Troquei a saída de som, mas um programa continuou no dispositivo antigo</b></summary>

A maioria dos programas segue a troca na hora. Alguns, principalmente jogos mais antigos, só passam a
usar o novo dispositivo depois de reabertos. O mesmo acontece quando a troca é feita pelo Windows.

</details>

Encontrou outro problema? [Abra uma issue](https://github.com/Igorms27/Islandbar/issues/new/choose)
com a versão (rodapé do painel) e os passos para ele acontecer.

## Para desenvolvedores

Feito em Rust com [egui/eframe](https://github.com/emilk/egui) e o crate
[windows](https://github.com/microsoft/windows-rs).

```bash
cargo run --release
```

O executável fica em `target\release\islandbar.exe`. Fora do Windows, o programa compila com dados de
mentira (`src/backend/mock.rs`), útil para mexer na interface.

| Arquivo | O que tem |
| --- | --- |
| `src/main.rs` | Cria a janela transparente, sem borda, sempre no topo. |
| `src/app.rs` | A ilha: estados (pílula, aviso, painel, alarme), animações e desenho. |
| `src/anim.rs` | A "mola" que dá o efeito elástico. |
| `src/alarm.rs` | Alarme: contagem e o bip de despertador. |
| `src/settings.rs` | Escolhas do cartão inicial (números da ilha, sensor da temperatura e pet). |
| `src/pet.rs` | Os pets (fantasma, robô e as barrinhas): desenho e gestos. |
| `src/delivery.rs` | Pedidos do iFood: recebe o andamento da extensão e guarda o pedido mostrado. |
| `src/map.rs` | Mapa da janela do pedido: baixa e posiciona os pedacinhos do mapa. |
| `extension/` | A extensão do navegador que acompanha os pedidos do iFood. Os nomes dos eventos do site vieram do projeto [iFood PiP](https://github.com/luannsr12/ifood-pip) (Apache 2.0); a consulta de pedidos em andamento, do [ifood-mcp](https://github.com/davidmosiah/ifood-mcp). |
| `src/platform.rs` | Mouse, teclado, janela sem moldura, memória e início com o Windows. |
| `src/updater.rs` | Atualização automática. |
| `src/perf.rs` | Medidor interno de quadros, CPU e memória (diagnóstico). |
| `src/backend/windows.rs` | Volume, dispositivos de som e controle de mídia do Windows. |
| `src/backend/sysmon.rs` | CPU, GPU, RAM e temperatura. |
| `src/backend/game.rs` | Modo jogo: detecta um jogo em tela cheia. |
| `src/backend/fps.rs` | Mede o FPS de outros programas. |
| `src/backend/mock.rs` | Dados de mentira para testar fora do Windows. |
| `examples/fake_game.rs` | "Jogo falso" usado pelo teste visual. |
| `scripts/smoke.ps1` | Teste visual: abre a ilha num Windows real do GitHub e tira screenshots. |

Como testar, o fluxo de versões e as regras do código estão em [CONTRIBUTING.md](CONTRIBUTING.md).

## Licença

Distribuído sob a licença **GNU GPL v3.0 ou posterior**: você pode usar, estudar, modificar e
redistribuir, desde que as versões modificadas que você distribuir também sejam GPL e com o código
aberto. Veja [LICENSE](LICENSE).
