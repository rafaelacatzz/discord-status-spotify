/**
 * build.mjs — Script de build para Discord Lyrics Status
 *
 * Uso:
 *   node build.mjs           → Compila backend + copia public/ para dist/
 *   node build.mjs bundle    → Compila backend apenas (usado pelo `npm run electron`)
 *   node build.mjs electron  → Build completo + empacotamento Electron (electron-builder)
 *
 * Dependências:
 *   esbuild ^0.25.0  (devDependency)
 *   electron-builder ^25.0.0 (devDependency, apenas para `electron` mode)
 */

import { build as esbuild } from 'esbuild';
import { copyFileSync, cpSync, existsSync, mkdirSync, readdirSync, rmSync, statSync } from 'fs';
import { resolve, dirname, relative, basename } from 'path';
import { fileURLToPath, pathToFileURL } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = __dirname;
const DIST = resolve(ROOT, 'dist');
const PUBLIC_SRC = resolve(ROOT, 'public');
const PUBLIC_DIST = resolve(DIST, 'public');

// ── Helpers ─────────────────────────────────────────────────────────────

function log(step, msg) {
  console.log(`[${step}] ${msg}`);
}

function copyRecursive(src, dest) {
  if (!existsSync(src)) {
    log('COPY', `⚠️  Source not found: ${src}`);
    return;
  }

  if (!existsSync(dest)) {
    mkdirSync(dest, { recursive: true });
  }

  const entries = readdirSync(src, { withFileTypes: true });
  for (const entry of entries) {
    const srcPath = resolve(src, entry.name);
    const destPath = resolve(dest, entry.name);

    if (entry.isDirectory()) {
      copyRecursive(srcPath, destPath);
    } else {
      copyFileSync(srcPath, destPath);
    }
  }

  const count = countFiles(dest);
  log('COPY', `Copied ${count} file(s) to ${relative(ROOT, dest)}`);
}

function countFiles(dir) {
  let count = 0;
  if (!existsSync(dir)) return 0;
  const entries = readdirSync(dir, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      count += countFiles(fullPath);
    } else {
      count++;
    }
  }
  return count;
}

function cleanDir(dir) {
  if (existsSync(dir)) {
    rmSync(dir, { recursive: true, force: true });
    log('CLEAN', `Removed ${relative(ROOT, dir)}`);
  }
}

// ── Build: backend bundle ───────────────────────────────────────────────

async function buildBackend() {
  log('BUILD', 'Bundling backend (src/ → dist/backend.cjs)...');

  const start = performance.now();

  await esbuild({
    entryPoints: [resolve(ROOT, 'src', 'index.js')],
    outfile: resolve(DIST, 'backend.cjs'),
    platform: 'node',
    target: ['node18'],
    format: 'cjs',
    bundle: true,
    external: [
      'electron',
    ],
    sourcemap: false,
    minifyWhitespace: true,
    minifyIdentifiers: false,  // keep readable stack traces
    keepNames: true,
    legalComments: 'none',
  });

  const elapsed = (performance.now() - start).toFixed(0);
  const size = statSync(resolve(DIST, 'backend.cjs')).size;
  log('BUILD', `Backend bundled (${(size / 1024).toFixed(1)} KB) in ${elapsed}ms`);
}

// ── Build: copy public/ ────────────────────────────────────────────────

function buildPublic() {
  log('BUILD', 'Copying public/ → dist/public/...');

  if (!existsSync(PUBLIC_SRC)) {
    log('BUILD', '⚠️  public/ directory not found — skipping');
    return;
  }

  // Clean dest first
  if (existsSync(PUBLIC_DIST)) {
    rmSync(PUBLIC_DIST, { recursive: true, force: true });
  }
  mkdirSync(PUBLIC_DIST, { recursive: true });

  copyRecursive(PUBLIC_SRC, PUBLIC_DIST);
}

// ── Build: Electron package ─────────────────────────────────────────────

async function ensureIconsExist() {
  const icoPath = resolve(ROOT, 'build', 'icon.ico');
  const pngPath = resolve(ROOT, 'build', 'icon.png');

  if (existsSync(icoPath)) {
    log('ICONS', 'Using existing icon.ico from build/');
    return;
  }

  if (existsSync(pngPath)) {
    log('ICONS', 'icon.ico not found, generating from existing icon.png...');
  } else {
    log('ICONS', 'Icons not found — auto-generating...');
  }
  try {
    const { generateIcons } = await import(pathToFileURL(resolve(ROOT, 'scripts', 'generate-icons.js')).href);
    await generateIcons();
    log('ICONS', 'Icons generated successfully');
  } catch (err) {
    log('ICONS', `⚠️  Auto-generation failed: ${err.message}. Continuing with placeholder...`);
  }
}

async function buildElectron() {
  log('BUILD', 'Starting Electron build (electron-builder)...');

  // Ensure dist/ is fully built first
  await buildBackend();
  buildPublic();

  // Ensure icons exist before running electron-builder
  await ensureIconsExist();

  // ── Limpa dist-electron ANTES de empacotar ────────────────────────────
  // O app minimiza para a bandeja ao fechar (mainWindow.on('close') esconde
  // em vez de sair). Se o usuário rodou um build anterior e o app ainda está
  // em execução, o .exe fica TRAVADO e o rcedit (etapa que grava ícone/versão
  // no executável) falha com "Fatal error: Unable to commit changes".
  // Limpar antes evita artefatos stale e detecta o problema logo no início,
  // com uma mensagem clara em vez de um erro obscuro no meio do build.
  const electronOut = resolve(ROOT, 'dist-electron');
  if (existsSync(electronOut)) {
    try {
      rmSync(electronOut, { recursive: true, force: true });
      log('CLEAN', `Removed ${relative(ROOT, electronOut)}`);
    } catch (err) {
      console.error('[BUILD] ⚠️  Não foi possível limpar dist-electron.');
      console.error('[BUILD]    O app Discord Lyrics Status ainda está em execução?');
      console.error('[BUILD]    Encerre todos os processos com:');
      console.error('[BUILD]      npm run kill-app');
      console.error('[BUILD]    (ou feche pela bandeja do sistema: tray → Sair)');
      console.error('[BUILD]    Detalhe: ' + (err.message || err));
      process.exit(1);
    }
  }

  // Run electron-builder
  log('BUILD', 'Running electron-builder...');
  const start = performance.now();

  // ── Assinatura digital ────────────────────────────────────────────────
  // O hook de assinatura SÓ é registrado quando há certificado configurado
  // (WIN_CSC_LINK / WIN_CSC_NAME / Azure). Sem ele, o electron-builder nem
  // tenta assinar — some o spam de ~20 avisos "signtool.exe not found" (um
  // por DLL/.exe) e as linhas "• signing with signtool.exe path=...".
  const hasSigningConfig = !!(
    process.env.WIN_CSC_LINK ||
    process.env.CSC_LINK ||
    process.env.WIN_CSC_NAME ||
    process.env.CSC_NAME ||
    process.env.AZURE_KEY_VAULT_URI
  );
  const skipSign = process.env.SKIP_SIGN === 'true' || process.env.SKIP_SIGN === '1';
  const signingEnabled = hasSigningConfig && !skipSign;

  if (skipSign) {
    log('BUILD', 'ℹ️  SKIP_SIGN=true — skipping digital signature.');
  } else if (!hasSigningConfig) {
    log('BUILD', 'ℹ️  No code-signing configured — installer will be UNSIGNED.');
    log('BUILD', '    (Set WIN_CSC_LINK or WIN_CSC_NAME to sign — see SIGNING.md)');
  }

  try {
    const { build: electronBuild } = await import('electron-builder');

    await electronBuild({
      projectDir: ROOT,
      config: {
        directories: {
          output: 'dist-electron',
          buildResources: 'build',
        },
        // asar ligado (recomendado): carrega mais rápido e evita caminhos
        // longos. O backend e o frontend são extraídos para app.asar.unpacked
        // porque o backend é iniciado via utilityProcess.fork e serve o
        // public/ com express/fs em tempo de execução.
        asar: true,
        asarUnpack: ['dist/**/*'],
        // ── Otimização de tamanho do instalador ─────────────────────────
        // compression maximum: LZMA mais forte no payload → instalador
        // ~5-10% menor (build um pouco mais lento, compensa na distribuição).
        compression: 'maximum',
        // Só en-US + pt-BR: as 55 locales do Chromium custavam ~41 MB
        // (~13 MB comprimidos) e o app é todo em HTML/PT-BR. O en-US fica
        // como fallback interno do Chromium.
        electronLanguages: ['en-US', 'pt-BR'],
        // Sem módulos nativos no app: desativa rebuild desnecessário no build.
        npmRebuild: false,
        nodeGypRebuild: false,
        files: [
          'electron/**/*',
          'dist/backend.cjs',
          'dist/public/**/*',
          'package.json',
          'build/icon.ico',
        ],
        win: {
          target: ['nsis'],
          icon: 'build/icon.ico',
          artifactName: '${productName}-Setup-${version}.exe',
          // Assinatura digital via scripts/sign.js (electron-builder v25+ usa
          // signtoolOptions). Só é ligado quando há certificado configurado;
          // caso contrário o electron-builder nem tenta assinar (build limpo).
          ...(signingEnabled
            ? {
                signtoolOptions: {
                  sign: resolve(ROOT, 'scripts', 'sign.js'),
                },
                // Assina DLLs e outros binários além de .exe
                signExts: ['.dll', '.node', '.exe'],
              }
            : {}),
        },
        nsis: {
          oneClick: false,
          perMachine: false,
          allowToChangeInstallationDirectory: true,
          deleteAppDataOnUninstall: false,
          createDesktopShortcut: true,
          createStartMenuShortcut: true,
          shortcutName: 'Discord Lyrics Status',
          installerIcon: 'build/icon.ico',
          uninstallerIcon: 'build/icon.ico',
          installerHeaderIcon: 'build/icon.ico',
          // EULA — exibido como página "License Agreement" no instalador
          license: 'build/eula.txt',
          // NSIS include — startup checkbox + custom settings page
          include: 'build/nsis-custom.nsh',
          // Cabeçalho personalizado (bitmap 164x314) — gerado em generate-icons.js
          installerHeader: existsSync(resolve(ROOT, 'build', 'installer-header.bmp'))
            ? 'build/installer-header.bmp'
            : undefined,
          // Nome no desinstalador do Windows
          uninstallDisplayName: 'Discord Lyrics Status ${version}',
          runAfterFinish: true,

          artifactName: '${productName}-Setup-${version}.exe',
        },
        publish: {
          provider: 'github',
          owner: 'rafaelacatzz',
          repo: 'discord-status-spotify',
          releaseType: 'release',
        },
        extraResources: [
          { from: 'build/icon.ico', to: 'build/icon.ico' },
          ...(existsSync(resolve(ROOT, 'smtc-daemon', 'SmtcDaemon.exe'))
            ? [{ from: 'smtc-daemon/SmtcDaemon.exe', to: 'smtc-daemon/SmtcDaemon.exe' }]
            : []),
        ],
      },
    });

    const elapsed = (performance.now() - start).toFixed(0);
    log('BUILD', `Electron package complete in ${elapsed}ms (output: dist-electron/)`);
  } catch (err) {
    console.error('[BUILD] electron-builder failed:', err.message || err);
    process.exit(1);
  }
}

// ── Main ────────────────────────────────────────────────────────────────

async function main() {
  const args = process.argv.slice(2);
  const mode = args[0] || 'default';

  log('BUILD', `Mode: ${mode === 'default' ? 'full build' : mode}`);

  // Ensure dist/ directory exists
  if (!existsSync(DIST)) {
    mkdirSync(DIST, { recursive: true });
  }

  switch (mode) {
    case 'electron':
      // Full build + electron-builder packaging
      await buildElectron();
      break;

    case 'bundle':
      // Bundle backend only (used by `npm run electron` dev workflow)
      await buildBackend();
      break;

    default:
      // Full build: bundle backend + copy public/
      await buildBackend();
      buildPublic();
      log('BUILD', '✅ Build complete!');
      log('BUILD', `   Backend:  dist/backend.cjs`);
      log('BUILD', `   Frontend: dist/public/`);
      break;
  }
}

main().catch((err) => {
  console.error('[BUILD] Fatal error:', err);
  process.exit(1);
});
