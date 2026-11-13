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
- A janela é recortada no formato da ilha: fora dela não existe nada na tela.

## Como rodar

1. Instale o Rust: <https://rustup.rs>
2. No terminal, dentro da pasta do projeto:

```bash
cargo run --release
```

O executável final fica em `target\release\islandbar.exe`.

Sem instalar nada: cada push no GitHub gera um `islandbar.exe` pronto na aba
**Actions** do repositório (artefato `islandbar-windows`).

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
| `src/platform.rs` | Mouse, recorte da janela e inicialização com o Windows. |

Feito com [egui/eframe](https://github.com/emilk/egui) e o crate
[windows](https://github.com/microsoft/windows-rs).
