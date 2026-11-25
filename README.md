<p align="center">
  <img src="build/icon.png" width="120" alt="Logo do SpotifySync" />
</p>

<h1 align="center">SpotifySync</h1>

<p align="center">
  <b>Sua música no status do Discord, linha por linha.</b><br />
  Aplicativo desktop nativo em Rust para Windows: lê o que toca no Spotify e
  transforma a letra sincronizada no seu status.
</p>

<p align="center">
  <a href="https://github.com/rafaelacatzz/discord-status-spotify/releases/latest">
    <img src="https://img.shields.io/github/v/release/rafaelacatzz/discord-status-spotify?style=for-the-badge&label=release&color=1ed760" alt="Release" />
  </a>
  <a href="https://github.com/rafaelacatzz/discord-status-spotify/releases">
    <img src="https://img.shields.io/github/downloads/rafaelacatzz/discord-status-spotify/total?style=for-the-badge&label=downloads&color=1ed760" alt="Downloads" />
  </a>
  <a href="LICENSE">
    <img src="https://img.shields.io/badge/license-MIT-1ed760?style=for-the-badge" alt="Licença MIT" />
  </a>
  <img src="https://img.shields.io/badge/Windows-10%20%7C%2011-1ed760?style=for-the-badge&logo=windows&logoColor=white" alt="Windows 10 e 11" />
  <img src="https://img.shields.io/badge/Rust-native-1ed760?style=for-the-badge&logo=rust&logoColor=white" alt="Rust" />
</p>

---

## 📥 Baixar

Pegue o executável mais recente na página de **[Releases](https://github.com/rafaelacatzz/discord-status-spotify/releases/latest)**:

| Arquivo | O que vem dentro | Indicado para |
| :--- | :--- | :--- |
| 📦 `SpotifySync-windows-x64.zip` | Executável + licença + leia-me | **Recomendado** |
| ⚡ `SpotifySync.exe` | Só o executável, sem instalação | Quem quer apenas rodar |

**Requisitos:** Windows 10 ou 11 (64-bit) e o **Spotify** instalado e aberto.
Não precisa instalar nada: extraia o `.exe` em qualquer pasta e execute.

> [!NOTE]
> O aplicativo não tem assinatura digital, então o Windows pode mostrar
> *"O Windows protegeu o computador"*. Clique em **Mais informações → Executar
> assim mesmo**. Todo o código-fonte está neste repositório.

---

## ✨ Funcionalidades

<details open>
<summary><b>🎵 Reprodução e letras</b></summary>

- 🪟 **Sem API do Spotify:** a reprodução é lida direto do **SMTC do Windows** — sem login do Spotify.
- 📝 **Letras do LRCLIB:** sincronizadas quando disponíveis e distribuídas pela duração quando só há texto simples.
- ⏱️ **Sincronização estável:** o relógio interpola a posição entre as leituras do SMTC e ignora leituras atrasadas ou congeladas, então o status **não pisca nem volta para a linha anterior**. Só um seek real move as letras para trás.
- 🥁 **Adiantamento inteligente:** estima o BPM pelas marcações das letras sincronizadas, ajusta a antecipação em frações de batida e limita o avanço para não pular linhas próximas. Letras sem timestamps não entram na estimativa.

</details>

<details open>
<summary><b>💬 Status no Discord e no League</b></summary>

- 🎧 **Status automático:** mostra a linha da letra atual e cai para `Título — Artista` quando a faixa não tem letra.
- 😀 **Emotes personalizados:** um para a letra e outro para título/artista, com prévia, presets e suporte a emotes de servidor (`nome:id` ou `<:nome:id>`).
- 🟢 **Presença configurável:** online, ausente, não perturbe ou invisível.
- 🎮 **League of Legends:** opção de espelhar a mesma mensagem no cliente do LoL — e um interruptor separado para o Discord, cada um ligado ou desligado quando quiser.
- 🎚️ **Ajustes finos:** adiantamento da letra em ms, horário das linhas e modo econômico.

</details>

<details open>
<summary><b>🎨 Interface e extras</b></summary>

- 🖼️ **Capa do álbum** via Deezer com fallback para iTunes; a paleta da interface muda junto com a música e mantém um tema neutro quando a capa é acinzentada.
- 🪄 **Interface nativa em egui:** início com prévia do status, página de letras com destaque da linha atual, configurações com prévia ao vivo e histórico local das 50 faixas mais recentes.
- 🔐 **Tela de conexão repaginada**, com prévia do status e passo a passo para o token.
- 💾 **Sessão lembrada** entre execuções (token validado antes de ser salvo).
- 🔄 **Atualização automática:** ao abrir e a cada 6 horas o app consulta o release mais recente, baixa o novo `SpotifySync.exe`, substitui o executável e reabre já na versão nova.

</details>

---

## 🚀 Como usar

1. **Abra o Spotify** e dê play em uma música.
2. **Abra o SpotifySync** e cole seu token do Discord na tela de conexão:

   | Passo | Ação |
   | :---: | :--- |
   | 1 | Abra o Discord no navegador e entre na sua conta. |
   | 2 | Pressione `F12` e abra a aba **Rede (Network)**. |
   | 3 | Procure qualquer requisição para `discord.com/api`. |
   | 4 | Copie o valor do cabeçalho `authorization`. |

3. **Pronto** — o app valida o token, mostra sua conta e passa a atualizar o status sozinho. 🎉

As preferências ficam em `%APPDATA%\discord-lyrics-status\config.json` (a pasta é
reutilizada de versões anteriores). A página **Configurações** mostra o caminho,
abre a pasta e permite restaurar os padrões.

---

## 🔐 Segurança do token

O token do Discord é o único dado sensível do app e fica **somente neste computador**:

| Proteção | Como funciona |
| :--- | :--- |
| 🔒 **Criptografado em repouso** | Gravado em `%APPDATA%\discord-lyrics-status\token.bin` com o **Windows DPAPI**, que só descriptografa para o mesmo usuário na mesma máquina. O `config.json` nunca contém o token; configurações antigas em texto puro são migradas e limpas sozinhas. |
| 📡 **Enviado só ao Discord** | Vai apenas no cabeçalho `Authorization` de requisições para `discord.com`, sempre em HTTPS. O cliente recusa HTTP simples e não segue redirecionamentos, então o cabeçalho não vaza para outro host. Letras, capas e a CDN do Discord são consultadas sem autenticação. |
| 🙈 **Nunca é registrado** | Não há log, mensagem de erro ou telemetria com o token. |
| 🧹 **Apagado ao sair** | "Sair" sobrescreve o arquivo antes de removê-lo e zera o token da memória. |

O app também aceita `DISCORD_USER_TOKEN` no ambiente (ou num `.env` local) para
desenvolvimento; esse valor é usado em memória e **nunca** é gravado em disco.

---

## 🧱 Compilar do código-fonte

**Requisitos:** Rust stable (`rustup`) e Visual Studio Build Tools com o workload
**Desktop development with C++** e um Windows SDK.

```powershell
cargo run            # abre o app em modo debug
cargo test           # roda a suíte de testes
cargo build --release
```

- O executável fica em `target\release\SpotifySync.exe`.
- O `build-portable.bat` compila e copia para `dist\SpotifySync.exe`.
- O build usa CRT estático (`+crt-static`, veja `.cargo/config.toml`), então o `.exe`
  roda em qualquer Windows 10/11 x64 sem instalar o runtime do Visual C++.

---

## 🚢 Publicar uma versão

Há dois caminhos, e o script local funciona mesmo sem GitHub Actions:

```bash
# 1. Atualize a versão em Cargo.toml
# 2. Compile, empacote, crie a tag e publique o release com o .exe
./scripts/release.sh
```

O script roda os testes, compila em release, gera `dist\SpotifySync.exe` e
`dist\SpotifySync-windows-x64.zip`, cria o release da versão e anexa os dois
arquivos. Use `./scripts/release.sh --no-upload` para apenas compilar e empacotar.
A credencial vem de `GITHUB_TOKEN`, do `gh auth token` ou da credencial que o git
já usa para push.

Os workflows em `.github/workflows/` fazem o mesmo de forma automática: **CI** roda
`cargo test` e o build de release em cada push/PR, e **Release** roda ao enviar uma
tag `v*` e anexa o `.zip` e o `.exe` ao release. O app instalado lê o release mais
recente e se atualiza sozinho: basta a nova tag ter um `SpotifySync.exe`.

> [!IMPORTANT]
> O repositório precisa ser **público**: a checagem de atualização não envia token,
> e um repositório privado responde `404` para o app.

Se precisar manter o código privado, hospede o `.exe` e um manifesto em qualquer
URL pública e aponte o app para ela em `%APPDATA%\discord-lyrics-status\config.json`:

```json
{ "updateFeed": "https://exemplo.com/spotifysync.json" }
```

```json
{ "version": "1.8.0", "url": "https://exemplo.com/SpotifySync.exe" }
```

`updateFeed` também aceita a URL da API de um release do GitHub. A versão vem de
`tag_name` ou `version` e só é baixada quando for maior que a instalada. O mesmo
valor pode ser passado pela variável `SPOTIFYSYNC_UPDATE_FEED`.

A substituição é feita por `update\apply-update.ps1` em `%APPDATA%\discord-lyrics-status`:
ele espera o app fechar, tenta trocar o `.exe` por até 10 segundos e reabre o
programa. O resultado fica em `update\apply.log`, e uma tentativa por versão evita
que o app entre em loop de reinício.

> [!TIP]
> Se as execuções do Actions aparecerem como `startup_failure` com zero jobs,
> verifique **Settings → Billing** (minutos do Actions) ou a política de Actions
> da conta.

---

## 🗂️ Estrutura

| Caminho | Função |
| :--- | :--- |
| `rust/src/main.rs` | Aplicativo nativo: interface, SMTC, LRCLIB, capas, Discord e auto-update |
| `build.rs` | Ícone e metadados do executável Windows |
| `build/icon.ico` | Ícone incorporado ao executável |
| `build/icon.png` | Identidade visual da interface e do README |
| `Cargo.toml` | Dependências e metadados do pacote |
| `.cargo/config.toml` | Build estático do CRT no Windows |
| `build-portable.bat` | Compila e copia o executável para `dist\` |
| `scripts/release.sh` | Compila, cria a tag e publica o release |
| `.github/workflows/` | CI e publicação automática de releases |

---

## ⚠️ Aviso

A atualização do status usa um endpoint **não oficial** do Discord e autenticação
por token de usuário. Isso pode violar os Termos de Serviço do Discord. O token é
validado no Discord e salvo apenas neste computador; o app não o envia a webhooks
ou serviços de terceiros. **Use por sua conta e risco.**

---

## 📄 Licença

Distribuído sob a licença [MIT](LICENSE).

<p align="center">
  Feito com 🎧 e Rust — e uma letra sempre no ritmo certo.
</p>
