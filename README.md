# Discord Lyrics Status 🎵

**Letras do Spotify em tempo real exibidas como status personalizado do Discord.**

O app monitora o que está tocando no Spotify (via SMTC do Windows) e atualiza o seu status do Discord com a faixa atual, letra sincronizada, capa do álbum e progresso.

![Plataforma](https://img.shields.io/badge/Windows-10%2B-0078D6) ![Node](https://img.shields.io/badge/Node.js-18%2B-339933) ![Licença](https://img.shields.io/badge/licença-MIT-green)

---

## ✨ Funcionalidades

- 🎵 **Faixa atual no status do Discord** — título, artista e álbum
- 📝 **Letras sincronizadas em tempo real** — via [LRCLIB](https://lrclib.net), com fallback para o [Genius](https://genius.com) (não-sincronizadas)
- 🖼️ **Capa do álbum** — buscada no Deezer / iTunes / Last.fm
- ⏩ **Barra de progresso** no status, sincronizada com a reprodução
- 🖥️ **Daemon SMTC em C#** — lê a sessão de mídia do Windows, **sem precisar de chaves da API do Spotify**
- ⚙️ **Configuração pelo navegador** — painel local (`setup.html`) para validar o token
- 📦 **Instalador Windows** (NSIS) com ícone na bandeja

## 🛠️ Como funciona

```
Spotify → Windows SMTC → SmtcDaemon.exe (C#) → backend Node.js → API do Discord
                                              ↘ letras (LRCLIB/Genius) + capa (Deezer/iTunes/Last.fm)
```

O daemon C# consulta a sessão de mídia do Windows a cada segundo e o backend monta o status personalizado com letra sincronizada.

## ✅ Requisitos

- **Windows 10 ou 11** (necessário para o SMTC)
- **Node.js 18+** (para rodar a partir do código)
- **.NET SDK 8+** (apenas para compilar o daemon C#)
- Um **token de usuário do Discord**

> ⚠️ **Aviso:** o app usa o *user token* do Discord (extraído via DevTools), método não oficial. O uso pode violar os Termos de Serviço do Discord — use por sua conta e risco.

## 🚀 Instalação (a partir do código)

```bash
# 1. Instale as dependências
npm install

# 2. Configure o token do Discord
copy .env.example .env
# edite o .env e defina DISCORD_USER_TOKEN=seu_token_aqui

# 3. Compile o daemon C# (se o postinstall não fez)
npm run build:daemon

# 4. Rode
npm start          # ou: npm run dev (watch mode)
```

**Obtendo o token do Discord:**

1. Abra o Discord no navegador e pressione `F12` (DevTools)
2. Vá em **Network**, faça qualquer requisição e copie o cabeçalho `Authorization`
3. Cole no `.env` como `DISCORD_USER_TOKEN` — ou use o painel de setup local

## 📦 Instalador Windows

```bash
npm run build:electron        # build completo + empacotamento Electron
# ou
build-installer.bat           # build assistido (gera ícones, daemon e instalador)
```

O instalador NSIS sai em `dist-electron/`. Para assinatura digital, veja [`SIGNING.md`](SIGNING.md).

## 📁 Estrutura

```
src/                 Backend Node.js (status, letras, capa, servidor local)
  index.js           Ponto de entrada
  spotify.js         Leitura da faixa via daemon SMTC
  status.js          Atualização do status no Discord
  lyrics.js          Letras (LRCLIB + Genius)
  album-art.js       Capa do álbum (Deezer/iTunes/Last.fm)
  server.js          Dashboard local (WebSocket + API)
  setup.js           Validação/salvamento do token
electron/            Shell do app desktop (Electron)
smtc-daemon/         Daemon C# (SMTC do Windows)
public/              Frontend do dashboard (HTML/CSS/JS)
build/               Recursos do instalador (ícone, EULA, NSIS)
scripts/             Scripts auxiliares (ícones, assinatura, validação)
```

## 🧰 Stack

| Camada | Tecnologia |
| --- | --- |
| Backend | Node.js (ESM) + esbuild |
| Desktop | Electron + electron-builder (NSIS) |
| Daemon | C# / .NET 8 (SMTC) |
| Letras | LRCLIB, Genius |
| Capa | Deezer, iTunes, Last.fm |
| Realtime | WebSocket (`ws`), Express |

## 📄 Licença

MIT — veja os termos de uso no [`build/eula.txt`](build/eula.txt).
