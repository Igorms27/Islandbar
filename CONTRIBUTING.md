# Contribuindo com a Islandbar

Obrigado pelo interesse! Este guia explica como compilar, testar e enviar mudanças.

## Antes de começar

- Ao participar, siga o nosso [Código de Conduta](CODE_OF_CONDUCT.md): respeito e gentileza com todo mundo.

- Para **problemas** e **ideias**, abra uma [issue](https://github.com/Igorms27/Islandbar/issues/new/choose)
  usando um dos modelos.
- Para mudanças grandes, abra uma issue antes para conversar sobre a ideia.

## Ambiente

1. Instale o Rust: <https://rustup.rs> (versão estável).
2. Clone o repositório e rode:

   ```bash
   cargo run
   ```

No Windows, a ilha usa o sistema de verdade (volume, mídia, GPU...). Em outros sistemas, ela compila
com dados de mentira (`src/backend/mock.rs`), útil para trabalhar na interface.

## Regras do projeto

- **Leve acima de tudo.** Parada, a ilha dorme (cerca de 1 quadro por segundo) e só redesenha quando
  algo muda. Não adicione trabalho a cada quadro nem leituras frequentes do sistema sem necessidade,
  e leia só o que está aparecendo na tela. Antes e depois de mexer em desempenho, meça com
  `ISLANDBAR_DEBUG_PERF=1`.
- **Nunca roubar o foco**, principalmente de jogos. A ilha só pega o teclado quando você clica num
  campo de digitar.
- **Português**: comentários no código e textos da interface ficam em português.
- **Commits**: mensagens curtas, em inglês, no imperativo (ex.: `Add alarm snooze button`).

## Antes de enviar

O GitHub confere estes três pontos em todo envio, e o envio falha se algum não passar:

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

## Teste visual no Windows

A cada envio, o GitHub Actions (`.github/workflows/build.yml`) compila a ilha num Windows real e roda o
`scripts/smoke.ps1`. O script abre a ilha com OpenGL por software, simula o mouse e o teclado, abre um
"jogo falso" (`examples/fake_game.rs`), tira screenshots e registra o consumo. Os resultados ficam no
artefato `smoke` da execução e na branch `ci-results`.

## Versões

- **Todo envio para o `main` vira uma versão** (`build-N` em
  [Releases](https://github.com/Igorms27/Islandbar/releases)), e as ilhas instaladas se atualizam
  sozinhas em poucos minutos. Só envie para o `main` código testado.
- Para testar sem publicar, use uma branch com o nome `ci/<algo>`: ela passa pelas mesmas checagens e
  pelo teste visual, mas não gera versão.
- As notas de cada versão são montadas a partir das mensagens de commit.
- Mudanças só em documentação (arquivos `.md`, `docs/`, modelos de issue) não geram versão.

## Variáveis de diagnóstico

| Variável | O que faz |
| --- | --- |
| `ISLANDBAR_DEBUG_PERF=1` | A cada 5 s, escreve no stderr quadros/s, CPU e memória. |
| `ISLANDBAR_DEBUG_UI=1` | Registra mudanças de estado da interface e da ordem das janelas. |
| `ISLANDBAR_DEBUG_GAME=1` | Registra a detecção de jogos (com dados de mentira, simula um jogo). |
| `ISLANDBAR_DEBUG_SYSMON=1` | Registra as leituras de CPU, GPU, RAM e temperatura. |
| `ISLANDBAR_DEBUG_DEVICES=1` | Mostra dispositivos de som de mentira quando não há nenhum. |
| `ISLANDBAR_DEBUG_ALARM=<s>` | Marca um alarme para daqui a `<s>` segundos ao abrir. |
| `ISLANDBAR_DEBUG_FPS_DENIED=1` | Finge que falta a permissão de FPS. |
| `ISLANDBAR_DEBUG_SILENT=1` | Dados de mentira sem som tocando. |
| `ISLANDBAR_MODE=collapsed\|expanded\|hud` | Abre direto num estado (para screenshots). |

## Licença

Ao contribuir, você concorda que sua contribuição seja distribuída sob a mesma licença do projeto,
a [GNU GPL v3.0 ou posterior](LICENSE).
