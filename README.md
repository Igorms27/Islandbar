# Islandbar

Uma "Dynamic Island" para Windows, escrita em Rust. Fica no topo da tela, sempre
visível, e se expande com animações elásticas.

## O que já faz

- **Pílula** no topo com relógio, música atual e barrinhas que dançam com o som.
- **Mixer**: clique na ilha para abrir. Volume geral + volume de cada aplicativo,
  com medidor de áudio ao vivo dentro de cada slider. Clique no ícone para mutar.
- **Mídia**: título/artista do que está tocando e botões anterior / play-pause / próxima
  (funciona com Spotify, navegadores e qualquer app que apareça no controle de mídia do Windows).
- **Aviso de volume**: ao usar as teclas de volume do teclado, a ilha se estica e mostra o nível.
- **Monitor do sistema**: CPU, GPU, RAM e temperatura no painel (e CPU/GPU/RAM na pílula quando não há música).
  O uso da GPU é o mesmo número do Gerenciador de Tarefas. A temperatura usa o melhor sensor que o PC
  oferece sem instalar driver: zona térmica da placa-mãe, depois placa de vídeo, depois o disco principal
  (o rótulo indica qual). Se nenhum existir, aparece "n/d".
- **Modo jogo**: com um jogo em tela cheia em primeiro plano, uma pílula pequena aparece ao lado da ilha
  com FPS, CPU, GPU e RAM. Navegadores e players de vídeo em tela cheia não contam como jogo.
  O FPS é medido como na Xbox Game Bar, sem mexer no jogo; o Windows exige uma permissão para isso,
  que a ilha oferece ativar no painel (pede confirmação de administrador uma vez e vale depois de
  sair e entrar na conta).
- **Iniciar com o Windows**: ao abrir o executável, um cartão pergunta se a ilha deve abrir
  sozinha no login. Para mudar depois, é só abrir o executável de novo.
- **Atualização automática**: a ilha confere se saiu versão nova no GitHub, baixa, troca o
  executável e reinicia sozinha. O número da versão aparece no rodapé do painel.
- A janela é transparente, sem borda nem sombra; só a ilha aparece.

## Como rodar

1. Instale o Rust: <https://rustup.rs>
2. No terminal, dentro da pasta do projeto:

```bash
cargo run --release
```

O executável final fica em `target\release\islandbar.exe`.

Sem instalar nada: baixe o executável mais recente em
<https://github.com/Igorms27/Islandbar/releases/latest/download/islandbar.exe>.
Cada push na branch `main` publica uma versão nova automaticamente, e as ilhas
já instaladas se atualizam sozinhas.

Para fechar: abra a ilha e clique em **Sair** no canto inferior direito.

## Estrutura do código

| Arquivo | O que tem |
| --- | --- |
| `src/main.rs` | Cria a janela transparente, sem borda, sempre no topo. |
| `src/app.rs` | A ilha: estados (pílula / aviso / painel), animações e desenho. |
| `src/anim.rs` | A "mola" que dá o efeito elástico. |
| `src/backend/windows.rs` | Conversa com o Windows: WASAPI (volume) e controle de mídia. |
| `src/backend/mock.rs` | Dados de mentira para testar a interface fora do Windows. |
| `src/backend/sysmon.rs` | CPU, GPU, RAM e temperatura. |
| `src/backend/game.rs` | Modo jogo: detecta um jogo em tela cheia. |
| `src/backend/fps.rs` | Mede o FPS de outros programas. |
| `examples/fake_game.rs` | "Jogo falso" usado pelo teste visual. |
| `src/updater.rs` | Atualização automática. |
| `scripts/smoke.ps1` | Teste visual: o GitHub abre a ilha num Windows real e guarda screenshots na branch `ci-results`. |
| `src/platform.rs` | Mouse, recorte da janela e inicialização com o Windows. |

Feito com [egui/eframe](https://github.com/emilk/egui) e o crate
[windows](https://github.com/microsoft/windows-rs).
