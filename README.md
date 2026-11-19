# SpotifySync

Aplicativo desktop nativo em Rust para Windows que mostra no status do Discord a
música que você está ouvindo no Spotify — com a letra sincronizada, linha por
linha.

[![CI](https://github.com/rafaelacatzz/discord-status-spotify/actions/workflows/ci.yml/badge.svg)](https://github.com/rafaelacatzz/discord-status-spotify/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/rafaelacatzz/discord-status-spotify?label=release)](https://github.com/rafaelacatzz/discord-status-spotify/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

## Baixar

Baixe o executável mais recente na página de
[**Releases**](https://github.com/rafaelacatzz/discord-status-spotify/releases/latest):

- `SpotifySync-windows-x64.zip` — executável + licença + leia-me (recomendado)
- `SpotifySync.exe` — apenas o executável, sem instalação

Requisitos: **Windows 10 ou 11 (64-bit)** e o **Spotify** instalado e aberto.
Não é preciso instalar nada: extraia o `.exe` em qualquer pasta e execute.

> O aplicativo não tem assinatura digital, então o Windows pode exibir o aviso
> "O Windows protegeu o computador". Clique em **Mais informações → Executar
> assim mesmo**. O código-fonte completo está neste repositório.

## Funcionalidades

- Leitura da reprodução direto do SMTC do Windows — sem API do Spotify, sem login
  do Spotify.
- Letras do LRCLIB: sincronizadas quando disponíveis e distribuídas pela duração
  quando só há texto simples.
- Status automático: mostra a linha da letra atual e cai para `Título — Artista`
  quando a faixa não tem letra.
- Sincronização estável: o relógio interpola a posição entre as leituras do SMTC
  e ignora leituras atrasadas ou congeladas, então o status **não pisca nem volta
  para a linha anterior**. Só um seek real move as letras para trás.
- Emotes personalizados: um para a letra e outro para título/artista, com prévia,
  presets e suporte a emotes de servidor (`nome:id` ou `<:nome:id>`).
- Presença do Discord (online, ausente, não perturbe, invisível), adiantamento da
  letra em ms, horário das linhas e modo econômico.
- Capa do álbum via Deezer com fallback para iTunes, usada no tema do app.
- Interface nativa em egui: início com prévia do status, página de letras com
  destaque da linha atual, configurações com prévia ao vivo.
- Sessão lembrada entre execuções (token validado antes de ser salvo).

## Como usar

1. Abra o Spotify e dê play em uma música.
2. Abra o SpotifySync e cole seu token do Discord na tela de conexão:
   1. Abra o Discord no navegador e entre na sua conta.
   2. Pressione `F12` → aba **Rede (Network)**.
   3. Procure qualquer requisição para `discord.com/api`.
   4. Copie o valor do cabeçalho `authorization`.
3. O app valida o token, mostra sua conta e passa a atualizar o status sozinho.

As preferências ficam em
`%APPDATA%\discord-lyrics-status\config.json` (a pasta é reutilizada de versões
anteriores). A página **Configurações** mostra o caminho, abre a pasta e permite
restaurar os padrões.

## Segurança do token

O token do Discord é o único dado sensível do app e fica **somente neste
computador**:

- **Criptografado em repouso.** O token é gravado em
  `%APPDATA%\discord-lyrics-status\token.bin` com o **Windows DPAPI**, que só
  permite descriptografar para o mesmo usuário do Windows na mesma máquina. O
  `config.json` nunca contém o token; configurações antigas com o token em texto
  puro são migradas e limpas automaticamente.
- **Enviado só ao Discord.** O token só vai no cabeçalho `Authorization` de
  requisições para `discord.com`, sempre em HTTPS. O cliente HTTP recusa HTTP
  simples e não segue redirecionamentos, então o cabeçalho não pode vazar para
  outro host. Letras (LRCLIB), capas (Deezer/iTunes) e a CDN do Discord são
  consultadas sem nenhum cabeçalho de autenticação.
- **Nunca é registrado.** Não há log, mensagem de erro ou telemetria com o token.
- **Apagado ao sair.** "Sair" sobrescreve o arquivo antes de removê-lo e zera o
  token da memória.

O app também aceita `DISCORD_USER_TOKEN` no ambiente (ou um `.env` local) para
desenvolvimento; esse valor é usado em memória e **nunca** é gravado em disco.

## Compilar do código-fonte

Requisitos: Rust stable (`rustup`) e Visual Studio Build Tools com o workload
**Desktop development with C++** e um Windows SDK.

```powershell
cargo run
cargo test
cargo build --release
```

O executável fica em `target\release\SpotifySync.exe`. O script
`build-portable.bat` compila e copia o binário para `dist\SpotifySync.exe`.

O build usa CRT estático (`+crt-static`, veja `.cargo/config.toml`), então o
`.exe` roda em qualquer Windows 10/11 x64 sem instalar o runtime do Visual C++.

## Publicar uma versão

Há dois caminhos, e o script local funciona mesmo sem GitHub Actions:

```bash
# 1. Atualize a versão em Cargo.toml
# 2. Compile, empacote, crie a tag e publique o release com o .exe
./scripts/release.sh
```

O script roda os testes, compila em release, gera `dist\SpotifySync.exe` e
`dist\SpotifySync-windows-x64.zip`, cria o release da versão e anexa os dois
arquivos. Use `./scripts/release.sh --no-upload` para apenas compilar e
empacotar. A credencial vem de `GITHUB_TOKEN`, do `gh auth token` ou da
credencial que o git já usa para push.

Os workflows em `.github/workflows/` fazem o mesmo de forma automática: `CI` roda
`cargo test` e o build de release em cada push/PR, e `Release` roda ao enviar uma
tag `v*` e anexa o `.zip` e o `.exe` ao release. Se as execuções aparecerem como
`startup_failure` com zero jobs, verifique **Settings → Billing** (minutos do
Actions) ou a política de Actions da conta.

## Estrutura

```text
rust/src/main.rs      Aplicativo nativo: interface, SMTC, LRCLIB, capas e Discord
build.rs              Ícone e metadados do executável Windows
build/icon.ico        Ícone incorporado ao executável
build/icon.png        Identidade visual da interface
Cargo.toml            Dependências e metadados do pacote
.cargo/config.toml    Build estático do CRT no Windows
build-portable.bat    Compila e copia o executável para dist\
.github/workflows/    CI e publicação automática de releases
```

## Aviso

A atualização do status usa um endpoint não oficial do Discord e autenticação por
token de usuário. Isso pode violar os Termos de Serviço do Discord. O token é
validado no Discord e salvo apenas neste computador; o app não o envia a
webhooks ou serviços de terceiros. Use por sua conta e risco.

## Licença

[MIT](LICENSE).
