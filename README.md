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
- **Monitor do sistema**: CPU, RAM e temperatura no painel (e CPU/RAM na pílula quando não há música).
  A temperatura depende de o PC informar esse dado ao Windows; quando não informa, aparece "n/d".
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
| `src/backend/sysmon.rs` | CPU, RAM e temperatura. |
| `src/updater.rs` | Atualização automática. |
| `src/platform.rs` | Mouse, recorte da janela e inicialização com o Windows. |

Feito com [egui/eframe](https://github.com/emilk/egui) e o crate
[windows](https://github.com/microsoft/windows-rs).
