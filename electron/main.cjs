const { app, BrowserWindow, Tray, Menu, nativeImage, Notification, ipcMain, shell, utilityProcess, dialog } = require('electron');
const { resolve, join } = require('path');
const { execSync } = require('child_process');
const { existsSync, readFileSync, writeFileSync, mkdirSync, unlinkSync } = require('fs');
const { homedir } = require('os');
const http = require('http');

// ── Lock file path (mesmo do backend em src/lock.js) ─────────────────────────
const LOCK_FILE = join(
  process.env.APPDATA || join(homedir(), '.config'),
  'discord-lyrics-status',
  'instance.lock'
);

// ── Single Instance Lock ──────────────────────────────────────────────────────────
const gotTheLock = app.requestSingleInstanceLock();

if (!gotTheLock) {
  app.quit();
} else {

const IS_DEV = !app.isPackaged;

// ── Backend entry (bundled dist/ ou src/ ao vivo) ─────────────────────────────
const BACKEND_PATH = (() => {
  const candidates = [];
  // Em build com asar, o caminho REAL em disco (app.asar.unpacked) é o mais
  // seguro para utilityProcess.fork — evita depender do redirect interno do asar.
  if (process.resourcesPath) {
    candidates.push(resolve(process.resourcesPath, 'app.asar.unpacked', 'dist', 'backend.cjs'));
  }
  candidates.push(resolve(__dirname, '..', 'dist', 'backend.cjs'));
  candidates.push(resolve(__dirname, '..', 'src', 'index.js'));
  for (const p of candidates) {
    try {
      if (existsSync(p)) return p;
    } catch {}
  }
  return candidates[candidates.length - 1];
})();

// ── Shared URL file (escrito pelo backend, lido pelo Electron) ──────────────
const URL_FILE = join(
  process.env.APPDATA || join(homedir(), '.config'),
  'discord-lyrics-status',
  'server.url'
);

// ── Config file (mesmo do backend em src/config.js — preferências compartilhadas) ──
const CONFIG_FILE = join(
  process.env.APPDATA || join(homedir(), '.config'),
  'discord-lyrics-status',
  'config.json'
);

let mainWindow = null;
let tray = null;
let serverProcess = null;
let isQuitting = false;

/** true = X minimiza para a bandeja; false = X fecha o app de verdade (configurável na UI) */
let closeToTray = true;

/**
 * Lê a preferência "minimizar para a bandeja ao fechar" do config.json
 * compartilhado com o backend. Default: true (comportamento original).
 */
function loadCloseToTrayPreference() {
  try {
    if (existsSync(CONFIG_FILE)) {
      const data = JSON.parse(readFileSync(CONFIG_FILE, 'utf8'));
      if (typeof data.closeToTray === 'boolean') closeToTray = data.closeToTray;
    }
  } catch {}
}

/**
 * Persiste a preferência "minimizar para a bandeja ao fechar" no config.json
 * (mesmo arquivo que o backend usa — sem conflito, chaves diferentes).
 * @param {boolean} enabled
 */
function saveCloseToTrayPreference(enabled) {
  closeToTray = !!enabled;
  try {
    let data = {};
    if (existsSync(CONFIG_FILE)) {
      try { data = JSON.parse(readFileSync(CONFIG_FILE, 'utf8')); } catch {}
    }
    data.closeToTray = closeToTray;
    mkdirSync(join(CONFIG_FILE, '..'), { recursive: true });
    writeFileSync(CONFIG_FILE, JSON.stringify(data, null, 2), 'utf8');
  } catch {}
}

app.on('second-instance', (_event, _commandLine, _workingDirectory) => {
  if (mainWindow) {
    if (mainWindow.isMinimized()) mainWindow.restore();
    mainWindow.show();
    mainWindow.focus();
  }
});

function isBackendAlreadyRunning() {
  try {
    if (!existsSync(LOCK_FILE)) return false;

    const data = JSON.parse(readFileSync(LOCK_FILE, 'utf8'));
    if (!data || !data.pid) return false;

    // Consistente com cleanupStaleBackend: um PID vivo mas de OUTRO processo
    // foi reciclado pelo Windows — não é um backend nosso rodando.
    if (!isOurAppProcess(data.pid)) return false;

    try {
      process.kill(data.pid, 0);
      return true;
    } catch {
      try {
        const stdout = execSync(`tasklist /FI "PID eq ${data.pid}" /NH`, {
          encoding: 'utf8',
          timeout: 3000,
          windowsHide: true,
        });
        return stdout.includes(String(data.pid));
      } catch {
        return false;
      }
    }
  } catch {
    return false;
  }
}

/**
 * Confirma se um PID pertence ao NOSSO app antes de matá-lo (segurança contra
 * reutilização de PID no Windows). Verifica o nome do processo e, para node/
 * electron, também a linha de comando (só os deste projeto).
 * @param {number} pid
 * @returns {boolean}
 */
function isOurAppProcess(pid) {
  try {
    const stdout = execSync(`tasklist /FI "PID eq ${pid}" /FO CSV /NH`, {
      encoding: 'utf8',
      timeout: 3000,
      windowsHide: true,
    });
    // Formato CSV: "discord-lyrics-status.exe","1234","Console",...
    const m = stdout.match(/"([^"]+\.exe)"/i);
    const name = m ? m[1].toLowerCase() : '';
    if (!name) return false;

    // App empacotado / daemon SMTC: nome já é suficiente
    if (name === 'discord-lyrics-status.exe' || name === 'smtcdaemon.exe') return true;

    // node / electron: só mata se a linha de comando aponta para o projeto
    if (name === 'node.exe' || name === 'electron.exe') {
      let cmdLine = '';
      try {
        cmdLine = execSync(`wmic process where "ProcessId=${pid}" get CommandLine /value`, {
          encoding: 'utf8',
          timeout: 3000,
          windowsHide: true,
        });
      } catch {
        // wmic foi removido no Windows 11 22H2+ — fallback via PowerShell CIM
        try {
          cmdLine = execSync(
            `powershell -NoProfile -NonInteractive -Command "(Get-CimInstance Win32_Process -Filter 'ProcessId=${pid}').CommandLine"`,
            { encoding: 'utf8', timeout: 5000, windowsHide: true }
          );
        } catch {}
      }
      return /discord-lyrics-status|discord-status-spotify|backend\.cjs|src[\\/]index\.js/i.test(cmdLine);
    }

    return false;
  } catch {
    return false;
  }
}

/**
 * Encerra um backend ZUMBI de sessão anterior que ainda segura o instance.lock
 * (processo vivo segurando o lock, mas servidor HTTP morto — ex.: sessão que
 * travou, kill parcial, atualização). Sem isso, o backend novo falha no
 * acquireLock() e sai com code 0 → o diálogo de erro "Backend exited with
 * code 0 before sending ready signal".
 */
function cleanupStaleBackend() {
  try {
    if (!existsSync(LOCK_FILE)) return;

    const data = JSON.parse(readFileSync(LOCK_FILE, 'utf8'));
    if (!data || !data.pid) return;

    // Se o processo já morreu, só remove o lock stale
    let alive = false;
    try {
      process.kill(data.pid, 0);
      alive = true;
    } catch {
      try {
        const stdout = execSync(`tasklist /FI "PID eq ${data.pid}" /NH`, {
          encoding: 'utf8',
          timeout: 3000,
          windowsHide: true,
        });
        alive = stdout.includes(String(data.pid));
      } catch {}
    }

    if (!alive) {
      try { unlinkSync(LOCK_FILE); } catch {}
      console.log('[Electron] Lock stale removido (processo morto).');
      return;
    }

    // Processo vivo: só mata se for do nosso app (nunca um PID reutilizado).
    // Se o PID vivo NÃO é nosso app, foi RECICLADO pelo Windows (a instância
    // anterior morreu e outro processo reusou o PID). O lock é stale — remove
    // o arquivo para o backend novo poder adquirir. NUNCA mata o processo alheio.
    if (!isOurAppProcess(data.pid)) {
      console.log(`[Electron] Lock aponta para PID ${data.pid} (reciclado por outro processo) — removendo lock stale...`);
      try { unlinkSync(LOCK_FILE); } catch {}
      return;
    }

    console.log(`[Electron] Backend zumbi detectado (PID ${data.pid}) — encerrando...`);
    try {
      execSync(`taskkill /PID ${data.pid} /T /F`, {
        encoding: 'utf8',
        timeout: 5000,
        windowsHide: true,
      });
    } catch {}
    // Pequena pausa para o Windows liberar o lock file antes de remover
    try { unlinkSync(LOCK_FILE); } catch {}
    console.log('[Electron] Backend zumbi encerrado e lock limpo.');
  } catch {}
}

/**
 * Lê o final do backend.log (gravação do próprio backend ao falhar) para
 * incluir o motivo real no diálogo de erro — sem isso o usuário só via
 * "Backend exited with code 0" sem pista do porquê.
 * @returns {string}
 */
function readBackendLogTail(maxChars = 1200) {
  try {
    const logPath = join(
      process.env.APPDATA || join(homedir(), '.config'),
      'discord-lyrics-status',
      'backend.log'
    );
    if (!existsSync(logPath)) return '';
    const content = readFileSync(logPath, 'utf8');
    return content.slice(-maxChars).trim();
  } catch {
    return '';
  }
}

function getSharedBackendUrl() {
  try {
    if (existsSync(URL_FILE)) {
      return readFileSync(URL_FILE, 'utf8').trim();
    }
  } catch {}
  return 'http://127.0.0.1:18542';
}

/**
 * Verifica se um servidor responde de verdade em uma URL (probe HTTP em /api/status).
 * Sem isso, um server.url STALE de uma sessão anterior (crash/kill/update) fazia
 * a janela carregar uma URL morta — o clássico fundo preto sem nenhum conteúdo.
 * @param {string} url
 * @returns {Promise<boolean>}
 */
function isUrlAlive(url) {
  return new Promise((resolve) => {
    let u;
    try {
      u = new URL(url);
    } catch {
      resolve(false);
      return;
    }
    const req = http.get(
      { hostname: u.hostname, port: u.port, path: '/api/status', timeout: 2500 },
      (res) => {
        res.resume();
        resolve(res.statusCode === 200);
      }
    );
    req.on('timeout', () => { req.destroy(); resolve(false); });
    req.on('error', () => resolve(false));
  });
}

/**
 * Espera até o backend responder na URL compartilhada (ou o tempo acabar).
 * Usado pelo fallback anti-tela-preta: a página só é recarregada quando o
 * servidor estiver de pé de fato.
 * @param {number} [timeoutMs]
 * @returns {Promise<boolean>}
 */
async function waitForBackendAlive(timeoutMs) {
  const deadline = Date.now() + (timeoutMs || 30000);
  while (Date.now() < deadline) {
    if (await isUrlAlive(getSharedBackendUrl())) return true;
    await new Promise((r) => setTimeout(r, 500));
  }
  return false;
}

function getAppIcon() {
  // Tenta carregar o .ico personalizado do usuário
  const iconPaths = [
    resolve(process.resourcesPath, 'build', 'icon.ico'),
    resolve(__dirname, '..', 'build', 'icon.ico'),
    resolve(__dirname, '..', 'icons8-discord-100.ico'),
  ];
  for (const p of iconPaths) {
    try {
      if (existsSync(p)) {
        return nativeImage.createFromPath(p);
      }
    } catch {}
  }
  // Fallback: generate a simple 32x32 canvas icon
  const size = 32;
  const canvas = Buffer.alloc(size * size * 4);
  const cx = size / 2, cy = size / 2, r = size / 2 - 1;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const dx = x - cx, dy = y - cy;
      const dist = Math.sqrt(dx * dx + dy * dy);
      const idx = (y * size + x) * 4;
      if (dist <= r) {
        const t = dist / r;
        canvas[idx] = Math.round(88 + (71 - 88) * t);
        canvas[idx + 1] = Math.round(101 + (82 - 101) * t);
        canvas[idx + 2] = Math.round(242 + (196 - 242) * t);
        canvas[idx + 3] = 255;
      } else {
        canvas[idx + 3] = 0;
      }
    }
  }
  return nativeImage.createFromBuffer(canvas, { width: size, height: size });
}

function startBackend() {
  return new Promise((resolveUrl, reject) => {
    // Remove a URL compartilhada de sessões ANTERIORES: o fallback urlFileCheck
    // abaixo só deve aceitar o arquivo que o backend NOVO escrever. Se um
    // server.url velho (porta de outra sessão) ficasse no caminho, a janela
    // poderia abrir numa URL morta → tela preta.
    try { unlinkSync(URL_FILE); } catch {}

    // Auto-cura: limpa um backend zumbi que ainda segura o instance.lock.
    // Sem isso o fork novo sai imediatamente com code 0 (acquireLock falhou).
    cleanupStaleBackend();

    console.log(`[Electron] Starting backend via utilityProcess: ${BACKEND_PATH}`);

    let started = false;
    let retries = 0;
    let urlFileCheck = null;
    let startTimeout = null;
    const MAX_RETRIES = 1;

    const stopUrlFileCheck = () => {
      if (urlFileCheck) {
        clearInterval(urlFileCheck);
        urlFileCheck = null;
      }
    };

    // Timeout de start com deadline POR TENTATIVA: cada fork (incl. retry)
    // ganha 30s completos, em vez de dividir um orçamento único de 30s.
    const armStartTimeout = () => {
      if (startTimeout) clearTimeout(startTimeout);
      startTimeout = setTimeout(() => {
        stopUrlFileCheck();
        if (!started) reject(new Error('Backend start timeout'));
      }, 30000);
    };

    const forkBackend = () => {
      if (retries > 0) {
        cleanupStaleBackend();
        // Desmonta os listeners do fork anterior (já morto) para que um
        // 'error' tardio nunca rejeite a promise durante o retry.
        if (serverProcess) {
          serverProcess.removeAllListeners();
          serverProcess = null;
        }
      }

      serverProcess = utilityProcess.fork(BACKEND_PATH, [], {
        env: { ...process.env, ELECTRON_RUN: '1' },
        serviceName: 'discord-lyrics-status-backend',
      });
      armStartTimeout();

      serverProcess.on('message', (msg) => {
        if (msg.type === 'server-ready' && msg.url) {
          started = true;
          // Limpa o monitor de URL imediatamente — sem isso ele fica
          // sondando HTTP a cada 500ms até o timeout de 30s (vazamento).
          stopUrlFileCheck();
          resolveUrl(msg.url);
        }
      });

      serverProcess.on('exit', (code) => {
        console.log(`[Electron] Backend process exited with code ${code}`);
        if (!started) {
          // Saída prematura (ex.: lock zumbi) → limpa e tenta uma vez mais
          if (retries < MAX_RETRIES) {
            retries++;
            console.log(`[Electron] Retrying backend start (tentativa ${retries + 1}/2)...`);
            setTimeout(forkBackend, 400);
            return;
          }
          reject(new Error(`Backend exited with code ${code} before sending ready signal`));
        }
      });

      serverProcess.on('error', reject);
    };

    forkBackend();

    // Fallback: monitor URL file — só confia se o servidor RESPONDER de fato
    // (probe HTTP em /api/status). Sem o probe, um server.url stale resolveria
    // o start com uma URL morta → janela preta.
    urlFileCheck = setInterval(async () => {
      try {
        if (existsSync(URL_FILE)) {
          const url = readFileSync(URL_FILE, 'utf8').trim();
          if (url && (await isUrlAlive(url))) {
            started = true;
            stopUrlFileCheck();
            resolveUrl(url);
          }
        }
      } catch {}
    }, 500);
  });
}

function createWindow(url) {
  mainWindow = new BrowserWindow({
    width: 1200,
    height: 800,
    minWidth: 900,
    minHeight: 600,
    title: 'Discord Lyrics Status',
    icon: getAppIcon(),
    show: false,
    backgroundColor: '#0a0b0e',
    webPreferences: {
      preload: resolve(__dirname, 'preload.cjs'),
      nodeIntegration: false,
      contextIsolation: true,
      spellcheck: false,
      backgroundThrottling: true,
    },
  });

  mainWindow.loadURL(url);

  mainWindow.once('ready-to-show', () => {
    mainWindow.show();
  });

  // ── Anti-tela-preta: se a página principal falhar (URL morta / backend caiu
  // no meio do start), espera o backend responder e recarrega na URL correta.
  let failHandled = false;
  let failRetries = 0;
  mainWindow.webContents.on('did-fail-load', async (_event, errorCode, errorDescription, _validatedURL, isMainFrame) => {
    if (isQuitting || failHandled) return;
    if (!isMainFrame || errorCode === -3) return; // subrecurso/abort — ignora
    failHandled = true;
    console.error(`[Electron] Page failed to load (${errorCode}): ${errorDescription}`);
    const alive = await waitForBackendAlive(30000);
    if (alive && mainWindow && !mainWindow.isDestroyed()) {
      failHandled = false;
      failRetries = 0;
      mainWindow.loadURL(getSharedBackendUrl());
      // Se o primeiro load falhou, `ready-to-show` pode nunca ter disparado —
      // garante que a janela apareça após a recarga bem-sucedida.
      if (!mainWindow.isVisible()) mainWindow.show();
    } else if (!alive) {
      // Backend nao voltou em 30s: desiste para evitar loop infinito de reload.
      failRetries++;
      if (failRetries >= 2) {
        try {
          dialog.showErrorBox(
            'Discord Lyrics Status',
            'O backend não respondeu. Feche o app pela bandeja do sistema e abra novamente.'
          );
        } catch {}
      }
      failHandled = false;
    }
  });

  mainWindow.on('close', (event) => {
    // X com "minimizar para a bandeja" ATIVO → esconde (comportamento original).
    // X com a opção DESLIGADA → deixa fechar de verdade; o window-all-closed
    // abaixo encerra o app por completo (backend + lock + tray).
    if (!isQuitting && closeToTray) {
      event.preventDefault();
      mainWindow.hide();
    }
  });

  // ── Fullscreen: notifica o renderer quando o estado muda ─────────────
  mainWindow.on('enter-full-screen', () => {
    mainWindow.webContents.send('fullscreen-changed', true);
  });
  mainWindow.on('leave-full-screen', () => {
    mainWindow.webContents.send('fullscreen-changed', false);
  });

  if (IS_DEV) {
    mainWindow.webContents.openDevTools({ mode: 'detach' });
  }
}

function createTray() {
  // Cria ícone 16x16 para a bandeja (redimensiona o .ico se disponível)
  const trayIcon = getAppIcon();
  tray = new Tray(trayIcon);
  tray.setToolTip('Discord Lyrics Status');

  const menuItems = [
    {
      label: 'Abrir Dashboard',
      click: () => {
        if (mainWindow) {
          mainWindow.show();
          mainWindow.focus();
        }
      },
    },
    { type: 'separator' },
  ];

  menuItems.push({
    label: 'Sair',
    click: () => {
      isQuitting = true;
      app.quit();
    },
  });

  const contextMenu = Menu.buildFromTemplate(menuItems);
  tray.setContextMenu(contextMenu);

  tray.on('double-click', () => {
    if (mainWindow) {
      mainWindow.show();
      mainWindow.focus();
    }
  });
}

// ── IPC Handlers ─────────────────────────────────────────────────────────────

ipcMain.on('show-notification', (_event, { title, body }) => {
  if (Notification.isSupported()) {
    new Notification({ title, body, icon: getAppIcon() }).show();
  }
});

ipcMain.on('open-external', (_event, url) => {
  if (url && typeof url === 'string' && url.startsWith('http')) {
    shell.openExternal(url);
  }
});

ipcMain.on('minimize-to-tray', () => {
  if (mainWindow) {
    mainWindow.hide();
  }
});

ipcMain.on('toggle-fullscreen', () => {
  if (mainWindow) {
    mainWindow.setFullScreen(!mainWindow.isFullScreen());
  }
});

// ── Fechar para a bandeja vs. sair (Configurações → Desempenho) ────────────
ipcMain.on('set-close-to-tray', (_event, enabled) => {
  saveCloseToTrayPreference(enabled);
});

ipcMain.handle('get-close-to-tray', () => closeToTray);

// ── Version IPC ─────────────────────────────────────────────────────────────
ipcMain.handle('get-app-version', () => {
  return app.getVersion();
});

// ── App lifecycle (dentro do else para acessar as variáveis) ────────────────
app.whenReady().then(async () => {
  try {
    loadCloseToTrayPreference();
    createTray();

    // ── Backend existente vs. novo ─────────────────────────────────────
    // Só reutiliza o backend "já rodando" se a URL compartilhada RESPONDER
    // de verdade (probe HTTP). Lock/URL stale de uma sessão anterior (crash,
    // kill, atualização) fazia a janela carregar uma URL morta — a tela preta.
    const sharedUrl = getSharedBackendUrl();
    const existingAlive = await isUrlAlive(sharedUrl);

    let url;
    if (isBackendAlreadyRunning() && existingAlive) {
      console.log('[Electron] Backend já está rodando. Conectando-se ao servidor existente...');
      url = sharedUrl;

      if (Notification.isSupported()) {
        new Notification({
          title: 'Discord Lyrics Status',
          body: 'Conectado ao backend existente.',
          icon: getAppIcon(),
        }).show();
      }
    } else {
      // Auto-cura do lock: remove um backend zumbi ANTES de subir um novo —
      // evita o "Backend exited with code 0" (acquireLock falhou).
      cleanupStaleBackend();
      url = await startBackend();

      if (Notification.isSupported()) {
        new Notification({
          title: 'Discord Lyrics Status',
          body: 'Aplicativo rodando em segundo plano.',
          icon: getAppIcon(),
        }).show();
      }
    }

    createWindow(url);

  } catch (err) {
    console.error('[Electron] Failed to start:', err.message);
    const backendLog = readBackendLogTail();
    try {
      dialog.showErrorBox(
        'Discord Lyrics Status',
        'Falha ao iniciar o app: ' + err.message + '\n\n' +
        (backendLog ? ('Motivo do backend:\n' + backendLog + '\n\n') : '') +
        'Tente encerrar processos antigos com: npm run kill-app' +
        '\n(ou feche o app pela bandeja do sistema — tray → Sair) e abra novamente.'
      );
    } catch {}
    app.quit();
  }
});

app.on('window-all-closed', () => {
  // Se o X encerrou de verdade (closeToTray=false), sai do app por completo:
  // app.quit() dispara before-quit, que derruba o backend e libera o lock.
  if (!isQuitting && !closeToTray) {
    isQuitting = true;
    app.quit();
  }
});

app.on('before-quit', () => {
  isQuitting = true;
  if (serverProcess) {
    serverProcess.kill();
    serverProcess = null;
  }
});

app.on('activate', () => {
  if (mainWindow) {
    mainWindow.show();
  }
});

// ── Fecha o else do single-instance lock ─────────────────────────────────────
}
