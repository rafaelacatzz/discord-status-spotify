/**
 * sign.js — Custom signer for electron-builder
 *
 * Suporta múltiplos métodos de assinatura:
 *   1. Local .pfx/.p12  → via WIN_CSC_LINK / CSC_LINK (env vars)
 *   2. Windows cert store → via WIN_CSC_NAME / CSC_NAME (subject name)
 *   3. Azure Key Vault  → via env vars (AZURE_KEY_VAULT_URI etc.)
 *   4. Skip (dev)       → retorna sem assinar se SKIP_SIGN=true
 *
 * Uso no build.mjs (electron-builder v25+):
 *   win: {
 *     signtoolOptions: {
 *       sign: resolve(__dirname, 'scripts', 'sign.js'),
 *     },
 *   }
 *
 * Baseado em: https://www.electron.build/code-signing
 *             https://github.com/electron-userland/electron-builder
 */

import { execSync } from 'child_process';
import { existsSync, writeFileSync, unlinkSync } from 'fs';
import { resolve, basename } from 'path';
import { tmpdir } from 'os';

// ── Config from environment ──────────────────────────────────────────────

const SKIP_SIGN = process.env.SKIP_SIGN === 'true' || process.env.SKIP_SIGN === '1';
const WIN_CSC_LINK = process.env.WIN_CSC_LINK || process.env.CSC_LINK;
const WIN_CSC_KEY_PASSWORD = process.env.WIN_CSC_KEY_PASSWORD || process.env.CSC_KEY_PASSWORD;
const WIN_CSC_NAME = process.env.WIN_CSC_NAME || process.env.CSC_NAME;

const AZURE_KEY_VAULT_URI = process.env.AZURE_KEY_VAULT_URI;
const AZURE_CERT_NAME = process.env.AZURE_CERT_NAME;
const AZURE_TENANT_ID = process.env.AZURE_TENANT_ID;
const AZURE_CLIENT_ID = process.env.AZURE_CLIENT_ID;
const AZURE_CLIENT_SECRET = process.env.AZURE_CLIENT_SECRET;

// ── Logging helpers ──────────────────────────────────────────────────────

function info(msg) { console.log(`[SIGN] ${msg}`); }
function warn(msg) { console.warn(`[SIGN] ⚠️  ${msg}`); }

/** Lista de arquivos temporários para limpar ao final */
const _tempFiles = [];
process.on('exit', () => {
  for (const f of _tempFiles) {
    try { unlinkSync(f); } catch { /* ok */ }
  }
});

// ── Helpers ──────────────────────────────────────────────────────────────

/** Retorna o PATH do sistema (case-insensitive no Windows) */
function getSystemPath() {
  return process.env.Path || process.env.PATH || process.env.path || '';
}

/** Adiciona um diretório ao PATH */
function addToPath(dir) {
  const current = getSystemPath();
  if (!current.includes(dir)) {
    process.env.Path = dir + ';' + current;
  }
}

/** Envolve execSync com melhor tratamento de erros */
function runSignTool(cmd, filePath, timeout = 60000) {
  try {
    execSync(cmd, { stdio: 'pipe', timeout, windowsHide: true, maxBuffer: 10 * 1024 * 1024 });
    info(`✓ Signed: ${basename(filePath)}`);
  } catch (err) {
    const stderr = (err.stderr || '').toString().trim();
    const stdout = (err.stdout || '').toString().trim();
    const details = stderr || stdout || err.message;
    throw new Error(`signtool failed for ${basename(filePath)}: ${details}`);
  }
}

// ── Signing methods ──────────────────────────────────────────────────────

/**
 * Verifica se um arquivo já está assinado verificando a presença
 * de uma assinatura digital (PKCS7 / Authenticode).
 */
function isAlreadySigned(filePath) {
  try {
    execSync(
      `signtool verify /pa "${filePath}"`,
      { stdio: 'pipe', timeout: 10000, windowsHide: true }
    );
    return true;
  } catch {
    return false;
  }
}

/**
 * Assina usando signtool com um arquivo .pfx local.
 */
function signWithPfx(filePath, pfxPath, password, hashAlgo) {
  const cmd = [
    'signtool sign',
    `/fd ${hashAlgo}`,
    `/td ${hashAlgo}`,
    '/as',
    `/f "${pfxPath}"`,
    password ? `/p "${password}"` : '',
    '/tr "http://timestamp.digicert.com"',
    `"${filePath}"`,
  ].filter(Boolean).join(' ');

  info(`Signing with .pfx: ${basename(filePath)}`);
  runSignTool(cmd, filePath);
}

/**
 * Assina usando signtool com um certificado do Windows cert store
 * (especificado pelo subject name).
 */
function signWithStore(filePath, subjectName, hashAlgo) {
  const cmd = [
    'signtool sign',
    `/fd ${hashAlgo}`,
    `/td ${hashAlgo}`,
    '/as',
    `/n "${subjectName}"`,
    '/tr "http://timestamp.digicert.com"',
    `"${filePath}"`,
  ].filter(Boolean).join(' ');

  info(`Signing with cert "${subjectName}": ${basename(filePath)}`);
  runSignTool(cmd, filePath);
}

/**
 * Assina usando Azure Code Signing (via Azure Key Vault + jsign / AzureSignTool).
 * Requer: dotnet tool install --global AzureSignTool
 * Ou: npm install -g @azure/identity @azure/keyvault-certificates
 *
 * Para simplificar, usamos AzureSignTool (recomendado pela Microsoft).
 */
async function signWithAzure(filePath) {
  // Tenta AzureSignTool (recomendado)
  try {
    const cmd = [
      'AzureSignTool sign',
      `-kvu "${AZURE_KEY_VAULT_URI}"`,
      `-kvc "${AZURE_CERT_NAME}"`,
      `-kvi "${AZURE_CLIENT_ID}"`,
      `-kvs "${AZURE_CLIENT_SECRET}"`,
      `-kvtd "${AZURE_TENANT_ID}"`,
      '--file-digest sha256',
      '--timestamp-rfc3161 "http://timestamp.digicert.com"',
      '--timestamp-digest sha256',
      `"${filePath}"`,
    ].join(' ');

    info(`Signing with Azure Key Vault: ${basename(filePath)}`);
    execSync(cmd, { stdio: 'pipe', timeout: 120000, windowsHide: true });
    info(`✓ Signed (Azure): ${basename(filePath)}`);
    return;
  } catch (err) {
    warn(`AzureSignTool failed: ${err.message}`);
    warn('Falling back to built-in electron-builder signing...');
    // Se AzureSignTool não estiver instalado, deixa o electron-builder
    // usar o método padrão (env vars)
    throw err;
  }
}

// ── Custom sign handler (called by electron-builder) ─────────────────────
//
// Recebe um objeto:
//   {
//     path: string,          // caminho do arquivo a assinar
//     name: string,          // nome do arquivo
//     hash: string,          // algoritmo de hash
//     isNsis: boolean,       // true se é o instalador NSIS
//     computeSignToolArgs: Function,  // gera args padrao do signtool
//   }
//
// Deve retornar Promise<void>.
// Se não assinar, chama computeSignToolArgs() para usar o fluxo padrão.

export default async function sign(config) {
  const { path: filePath, isNsis, hash: algo } = config;
  const hashAlgo = (algo || 'sha256').toUpperCase(); // 'sha256' → 'SHA256'

  // ── Skip mode (dev / testing) ──────────────────────────────────────
  if (SKIP_SIGN) {
    info(`SKIP_SIGN=true — skipping: ${basename(filePath)}`);
    return;
  }

  // ── Nenhum método de assinatura configurado → build sem assinatura ──
  // Sem certificado, não há o que assinar — sai em silêncio (evita o spam
  // de "signtool.exe not found" para cada DLL/.exe em builds de dev).
  const hasSigningMethod = !!(
    WIN_CSC_LINK ||
    WIN_CSC_NAME ||
    (AZURE_KEY_VAULT_URI && AZURE_CERT_NAME)
  );
  if (!hasSigningMethod) {
    return;
  }

  // ── Verifica se signtool está disponível ───────────────────────────
  let signtoolAvailable = false;
  try {
    execSync('where signtool', { stdio: 'pipe', timeout: 5000, windowsHide: true });
    signtoolAvailable = true;
  } catch {
    // Tenta caminho comum do Windows SDK
    const possiblePaths = [
      'C:\\Program Files (x86)\\Windows Kits\\10\\bin\\10.0.22621.0\\x64\\signtool.exe',
      'C:\\Program Files (x86)\\Windows Kits\\10\\bin\\10.0.22000.0\\x64\\signtool.exe',
      'C:\\Program Files (x86)\\Windows Kits\\10\\bin\\10.0.20348.0\\x64\\signtool.exe',
      'C:\\Program Files (x86)\\Windows Kits\\10\\bin\\10.0.19041.0\\x64\\signtool.exe',
      'C:\\Program Files (x86)\\Microsoft SDKs\\Windows\\v10.0A\\bin\\NETFX 4.8 Tools\\signtool.exe',
    ];
    for (const p of possiblePaths) {
      if (existsSync(p)) {
        addToPath(p.replace('signtool.exe', ''));
        signtoolAvailable = true;
        break;
      }
    }
  }

  if (!signtoolAvailable) {
    warn('signtool.exe not found — skipping signing.');
    warn('Install Windows SDK or set SKIP_SIGN=true for dev builds.');
    return;
  }

  // ── Verifica se já está assinado ──────────────────────────────────
  if (isAlreadySigned(filePath)) {
    info(`Already signed, skipping: ${basename(filePath)}`);
    return;
  }

  // ── 1. Azure Key Vault ───────────────────────────────────────────
  if (AZURE_KEY_VAULT_URI && AZURE_CERT_NAME) {
    await signWithAzure(filePath);
    return;
  }

  // ── 2. Local .pfx ─────────────────────────────────────────────────
  if (WIN_CSC_LINK) {
    let pfxPath = WIN_CSC_LINK;

    // Suporta base64 encoded inline
    if (WIN_CSC_LINK.startsWith('base64://')) {
      const base64Data = WIN_CSC_LINK.slice('base64://'.length);
      const tempPfx = resolve(tmpdir(), `dls-cert-${Date.now()}.pfx`);
      writeFileSync(tempPfx, Buffer.from(base64Data, 'base64'));
      _tempFiles.push(tempPfx); // cleanup na saida
      pfxPath = tempPfx;
      info('Decoded base64 certificate to temporary file');
    }

    if (existsSync(pfxPath)) {
      signWithPfx(filePath, pfxPath, WIN_CSC_KEY_PASSWORD, hashAlgo);
      return;
    } else {
      warn(`Certificate file not found: ${pfxPath}`);
    }
  }

  // ── 3. Windows cert store ─────────────────────────────────────────
  if (WIN_CSC_NAME) {
    signWithStore(filePath, WIN_CSC_NAME, hashAlgo);
    return;
  }

  // ── Fallback: deixa o electron-builder tentar com os defaults ─────
  if (isNsis) {
    warn('No signing method configured. Installer will be UNSIGNED.');
    warn('See SIGNING.md for instructions on setting up code signing.');
    warn('');
    warn('To suppress this warning, set: SKIP_SIGN=true');
  }
}
