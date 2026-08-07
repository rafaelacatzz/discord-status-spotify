# 🔏 Assinatura Digital (Authenticode)

Este documento explica como configurar a **assinatura digital** do instalador para evitar os avisos do Windows SmartScreen ("Windows protected your PC" / "Unknown publisher").

---

## 📋 Visão Geral

O Windows exibe um aviso de segurança quando um usuário tenta instalar um `.exe` que **não possui assinatura digital**. Para eliminar esse aviso, o instalador precisa ser assinado com um **certificado Authenticode** emitido por uma Autoridade Certificadora (CA) confiável.

### Tipos de certificado

| Tipo | Custo | SmartScreen | Uso |
|---|---|---|---|
| **Auto-assinado** | Grátis | ⚠️ Aviso aparece (não confiável para outros PCs) | Testes locais |
| **OV Code Signing** | ~R$200-400/ano | ✅ Sem aviso (após reputação) | Distribuição geral |
| **EV Code Signing** | ~R$600-1200/ano | ✅ Sem aviso (imediato) | Distribuição profissional |

> **Recomendação**: Para distribuir o app para outras pessoas, adquira um certificado OV Code Signing de uma CA como DigiCert, Sectigo, ou Certisign.

---

## 🛠️ Métodos de Assinatura

O projeto suporta **3 métodos** de assinatura, configurados via variáveis de ambiente:

### Método 1: Arquivo .pfx local (recomendado)

```bash
set WIN_CSC_LINK=C:\caminho\para\seu-certificado.pfx
set WIN_CSC_KEY_PASSWORD=senha-do-certificado
npm run build:installer
```

Ou em PowerShell:
```powershell
$env:WIN_CSC_LINK = "C:\caminho\para\seu-certificado.pfx"
$env:WIN_CSC_KEY_PASSWORD = "senha-do-certificado"
npm run build:installer
```

### Método 2: Certificado do Windows Certificate Store

Se o certificado estiver instalado no repositório do Windows (ex: USB token / HSM):

```bash
set WIN_CSC_NAME=Discord Lyrics Status
npm run build:installer
```

O `WIN_CSC_NAME` deve corresponder ao **Subject Name** (CN) do certificado.

### Método 3: Azure Key Vault (CI/CD)

Para pipelines de CI/CD usando Azure Code Signing:

```bash
set AZURE_KEY_VAULT_URI=https://meu-kv.vault.azure.net
set AZURE_CERT_NAME=meu-certificado
set AZURE_TENANT_ID=xxxx-xxxx-xxxx
set AZURE_CLIENT_ID=xxxx-xxxx-xxxx
set AZURE_CLIENT_SECRET=xxxx-xxxx-xxxx
npm run build:installer
```

Requer: `dotnet tool install --global AzureSignTool`

---

## 🧪 Testes com Certificado Auto-assinado

Para testar o pipeline de assinatura localmente (o certificado só será confiável no seu PC):

### Opção A: Script automático

```bash
scripts/sign-setup.bat
```

Este script:
1. Gera um certificado auto-assinado com OpenSSL ou PowerShell
2. Salva em `build/certs/test-cert.pfx`
3. Opcionalmente instala na store confiável do Windows (requer Admin)

### Opção B: Manual com PowerShell

```powershell
# Criar certificado auto-assinado
$cert = New-SelfSignedCertificate `
    -Subject "CN=Discord Lyrics Status,O=DiscordLyricsStatus,C=BR" `
    -Type CodeSigning `
    -CertStoreLocation "Cert:\CurrentUser\My" `
    -KeyUsage DigitalSignature `
    -KeyExportPolicy Exportable `
    -NotAfter (Get-Date).AddYears(10)

# Exportar como PFX
$password = ConvertTo-SecureString "minha-senha" -Force -AsPlainText
Export-PfxCertificate -Cert $cert -FilePath "build\certs\test-cert.pfx" -Password $password

# Exportar como CER (para instalar em outras máquinas)
Export-Certificate -Cert $cert -FilePath "build\certs\test-cert.cer" -Type CERT
```

### Usando o certificado de teste

```bash
set WIN_CSC_LINK=build\certs\test-cert.pfx
set WIN_CSC_KEY_PASSWORD=discord-lyrics-status-test-2026
npm run build:installer
```

Para **instalar o certificado** como confiável (elimina o aviso no seu PC):
```bash
certutil -addstore -f Root build\certs\test-cert.cer
certutil -addstore -f TrustedPublisher build\certs\test-cert.cer
```

> ⚠️ Certificados auto-assinados **não funcionam em outros PCs**. O SmartScreen continuará alertando outros usuários. Use apenas para testes.

---

## ☁️ CI/CD (GitHub Actions)

Adicione os segredos no repositório GitHub:

| Segredo | Valor |
|---|---|
| `WIN_CSC_LINK` | Conteúdo do .pfx em **base64** |
| `WIN_CSC_KEY_PASSWORD` | Senha do certificado |

Para converter o .pfx para base64 (PowerShell):
```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx")) | Set-Clipboard
```

No workflow `.github/workflows/build.yml`:
```yaml
- name: Build installer
  env:
    WIN_CSC_LINK: ${{ secrets.WIN_CSC_LINK }}
    WIN_CSC_KEY_PASSWORD: ${{ secrets.WIN_CSC_KEY_PASSWORD }}
  run: npm run build:installer
```

---

## 🔄 Fluxo de Signing (como funciona)

```
build.mjs electron
  │
  ├─ buildBackend()          → esbuild: src/ → dist/backend.cjs
  ├─ buildPublic()           → copia public/ → dist/public/
  ├─ ensureIconsExist()      → gera build/icon.ico se ausente
  │
  └─ electron-builder
       │
       ├── Empacota app (.exe)
       ├── Chama scripts/sign.js para CADA arquivo:
       │    • disco-lyrics-status.exe (app)
       │    • SmtcDaemon.exe (se incluso)
       │    • Discord Lyrics Status Setup 1.0.0.exe (instalador NSIS)
       │    • *.dll / *.node (se signExts configurado)
       │
       └── Saída: dist-electron/Discord Lyrics Status-Setup-1.0.0.exe
```

O script `scripts/sign.js` tenta nesta ordem:
1. Se `SKIP_SIGN=true` → pula (dev)
2. Se `AZURE_KEY_VAULT_URI` setado → Azure Code Signing
3. Se `WIN_CSC_LINK` setado → assina com .pfx local
4. Se `WIN_CSC_NAME` setado → assina com cert store
5. Se nada configurado → avisa que o installer não será assinado

---

## 🛑 Solução de Problemas

| Problema | Causa | Solução |
|---|---|---|
| `signtool.exe not found` | Windows SDK não instalado | Instale "Windows SDK" via Visual Studio Installer |
| `SignTool Error: No certificates were found` | PFX inválido ou senha errada | Verifique `WIN_CSC_LINK` e `WIN_CSC_KEY_PASSWORD` |
| `Access denied` | Certificado não exportável | Use `-KeyExportPolicy Exportable` ao gerar |
| `The timestamp service is not available` | Timestamp server bloqueado | Tente outro servidor: `http://timestamp.sectigo.com` |
| SmartScreen ainda alerta | Certificado novo sem reputação | Distribua por alguns meses ou use EV Code Signing |
| `AzureSignTool` não encontrado | Não instalado | `dotnet tool install --global AzureSignTool` |

---

## 🔗 Links Úteis

- [Comprar certificado DigiCert](https://www.digicert.com/code-signing/)
- [Comprar certificado Certisign](https://www.certisign.com.br/certificado-digital/code-signing)
- [Comprar certificado Sectigo](https://sectigo.com/ssl-certificates-tls/code-signing)
- [Documentação electron-builder sobre code signing](https://www.electron.build/code-signing)
- [Azure Code Signing](https://learn.microsoft.com/en-us/azure/security/fundamentals/code-signing-overview)
