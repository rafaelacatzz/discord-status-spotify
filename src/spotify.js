import { spawn, execFile } from 'child_process';
import { resolve } from 'path';
import { existsSync } from 'fs';
import * as ui from './ui.js';

// ── Caminho do daemon compilado ─────────────────────────────────────────
// Resolve LAZY (dentro de startDaemon) para evitar checar o FS no load
// do módulo. Isso permite que o usuário compile o daemon após a primeira
// execução sem precisar reiniciar o app.
//
// Ordem de busca:
//   1. process.cwd() + '/smtc-daemon/SmtcDaemon.exe'           (dev, root)
//   2. process.cwd() + '/../smtc-daemon/SmtcDaemon.exe'        (Electron: 1 nível acima)
//   3. process.cwd() + '/../../smtc-daemon/SmtcDaemon.exe'     (Electron: 2 níveis = raiz do projeto)
//   4. process.resourcesPath + '/smtc-daemon/SmtcDaemon.exe'   (Electron asar)
//
// O build:daemon copia o exe para project-root/smtc-daemon/SmtcDaemon.exe
function resolveDaemonPath() {
  // Caminhos candidatos, testados em ordem
  const candidates = [
    resolve(process.cwd(), 'smtc-daemon', 'SmtcDaemon.exe'),          // 0: CWD
    resolve(process.cwd(), '..', 'smtc-daemon', 'SmtcDaemon.exe'),    // 1 nível acima
    resolve(process.cwd(), '..', '..', 'smtc-daemon', 'SmtcDaemon.exe'), // 2 níveis acima
  ];

  if (process.resourcesPath) {
    candidates.push(resolve(process.resourcesPath, 'smtc-daemon', 'SmtcDaemon.exe'));
  }

  for (const path of candidates) {
    if (existsSync(path)) return path;
  }

  return candidates[0]; // fallback — vai falhar com ENOENT
}

// ── Callback de erro (registrado pelo index.js para broadcast ao dashboard) ──
let _onError = null;
export function setSmtcErrorHandler(handler) {
  _onError = handler;
}

// ── Estado compartilhado entre o processo daemon e as consultas ─────────
let latestData = null;
/** Timestamp do ÚLTIMO JSON emitido pelo daemon (qualquer estado) —
 *  sinal confiável de liveness. O daemon escreve a cada ~1s mesmo pausado
 *  ou sem sessão, então se isto estiver velho o daemon morreu. */
let lastDaemonOutputMs = 0;
/** Último dado válido (não nulo) — preservado mesmo após crash do daemon */
let lastValidData = null;
let daemonProcess = null;
let daemonAlive = false;
let buffer = '';
let restartAttempts = 0;
let hasEverHadData = false;
const MAX_RESTART_ATTEMPTS = 10;
const STARTUP_WAIT_MS = 5000;
let resolveStartup = null;
let startupPromise = null;
/** true = backend em encerramento — bloqueia novos spawns e restarts */
let shuttingDown = false;
/** Deduplica inícios concorrentes (getCurrentTrack + timers de restart) */
let startingPromise = null;
/** true = o último daemon saiu (code 0) sem nunca emitir dados → o próximo
 *  spawn desliga o self-heal do manager stale (SMTC_NO_SELF_HEAL=1). Evita
 *  loop de restart quando o Spotify está genuinamente fechado: o 2º processo
 *  fica no heartbeat de 2s. Resetado quando um daemon produz dados. */
let lastDaemonExitedWithNoData = false;

/**
 * Mata daemons SmtcDaemon.exe ÓRFÃOS — processos de backends que morreram
 * abruptamente (taskkill /F, crash) e ficaram monopolizando a sessão SMTC.
 * Um órfão vivo faz o daemon novo falhar com "Nenhuma instância disponível"
 * (o SMTC do Windows só dá acesso à sessão a um processo por vez) até o
 * órfão morrer — exatamente o sintoma visto: o daemon novo rodava, mas o
 * backend não obtinha dados até matarmos o órfão à mão.
 *
 * Usa tasklist/taskkill (Windows). Nunca mata o nosso próprio filho
 * (exclui daemonProcess.pid) e ignora falhas (é limpeza best-effort).
 */
function killOrphanDaemons() {
  return new Promise((resolve) => {
    const excludePid = daemonProcess?.pid;
    execFile('tasklist', ['/FI', 'IMAGENAME eq SmtcDaemon.exe', '/FO', 'CSV', '/NH'], { windowsHide: true }, (err, stdout) => {
      if (err || !stdout) return resolve();

      const pids = [];
      for (const line of stdout.split(/\r?\n/)) {
        // CSV: "SmtcDaemon.exe","1234","Console","1","12,345 K"
        const m = /"SmtcDaemon\.exe"\s*,\s*"(\d+)"/i.exec(line);
        if (!m) continue;
        const pid = Number(m[1]);
        if (pid && pid !== excludePid) pids.push(pid);
      }
      if (pids.length === 0) return resolve();

      let remaining = pids.length;
      for (const pid of pids) {
        execFile('taskkill', ['/F', '/PID', String(pid)], { windowsHide: true }, () => {
          if (--remaining === 0) {
            console.warn(`[SMTC] Killed ${pids.length} orphan daemon(s): ${pids.join(', ')}`);
            resolve();
          }
        });
      }
    });
  });
}

/**
 * Inicia o processo do daemon C# compilado.
 * O executável fica em loop, consultando SMTC a cada ~1s e escreve JSON no stdout.
 *
 * Antes do spawn, mata daemons órfãos (ver killOrphanDaemons) e passa o PID do
 * backend via SMTC_PARENT_PID — o daemon se auto-encerra se o pai morrer de
 * forma que o Node não detecta (crash nativo, kill -9).
 */
function startDaemon() {
  if (daemonProcess && daemonAlive) return;
  if (startingPromise) return startingPromise;
  startingPromise = doStartDaemon().finally(() => { startingPromise = null; });
  return startingPromise;
}

async function doStartDaemon() {
  if (shuttingDown) return;

  try {
    // 1) Limpeza de órfãos ANTES do spawn — ver nota em killOrphanDaemons.
    await killOrphanDaemons();

    if (shuttingDown) return;
    // Re-checa após o await: um start concorrente pode ter spawnado no meio.
    if (daemonProcess && daemonAlive) return;

    const daemonPath = resolveDaemonPath(); // lazy resolve a cada restart
    daemonProcess = spawn(daemonPath, [], {
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
      env: {
        ...process.env,
        // Watchdog anti-órfão: o daemon checa o pai a cada loop e sai sozinho
        // se o backend morrer sem executar os handlers de exit (taskkill /F).
        SMTC_PARENT_PID: String(process.pid),
        // Desliga o self-heal do manager stale após um exit sem dados — ver
        // lastDaemonExitedWithNoData (evita loop com Spotify fechado).
        SMTC_NO_SELF_HEAL: lastDaemonExitedWithNoData ? '1' : '0',
      },
    });

    daemonProcess.stdout.setEncoding('utf8');

    // Flag para logar quando o daemon começa a produzir dados após restart
    let firstDataAfterStart = true;

    daemonProcess.stdout.on('data', (chunk) => {
      buffer += chunk;

      // Processa linhas completas
      let newlineIdx;
      while ((newlineIdx = buffer.indexOf('\n')) !== -1) {
        const line = buffer.substring(0, newlineIdx).replace(/\r$/, '').trim();
        buffer = buffer.substring(newlineIdx + 1);

        if (!line) continue;

        // Marca liveness: qualquer linha emitida (mesmo {isPlaying:false})
        // prova que o daemon está vivo — NÃO usar data.lastUpdatedMs do SMTC,
        // que congela quando a música está pausada.
        lastDaemonOutputMs = Date.now();

        try {
          const parsed = JSON.parse(line);
          if (parsed && typeof parsed === 'object') {
            if (firstDataAfterStart) {
              firstDataAfterStart = false;
              console.log(`[SMTC] Daemon active`);
            }
            // Daemon saudável: reabilita o self-heal para o próximo restart.
            if (lastDaemonExitedWithNoData) lastDaemonExitedWithNoData = false;

            if (parsed.psFatal) {
              console.error('[SMTC-FATAL]', parsed.psFatal);
              if (_onError) _onError('smtc-fatal', parsed.psFatal);
            }
            if (parsed.psError) {
              console.warn('[SMTC-ERROR]', parsed.psError);
              if (_onError) _onError('smtc-error', parsed.psError);
            }
            latestData = parsed;
            // Preserva APENAS dados de faixa ATIVA (isPlaying=true + título)
            // Isto é CRÍTICO: não salvar {isPlaying:false} como lastValidData
            if (parsed.isPlaying && parsed.title) {
              lastValidData = parsed;
            }
            if (!hasEverHadData && parsed !== null) {
              hasEverHadData = true;
              if (resolveStartup) {
                resolveStartup();
                resolveStartup = null;
                startupPromise = null;
              }
            }
          }
        } catch {
          // Ignora linhas que não são JSON
        }
      }
    });

    daemonProcess.stderr.on('data', (chunk) => {
      const text = chunk.toString('utf8');
      console.error('[SMTC-DAEMON]', text);
      if (_onError) _onError('daemon', text.substring(0, 200));
    });

    daemonProcess.on('exit', (code) => {
      const hadData = hasEverHadData;
      daemonAlive = false;
      daemonProcess = null;
      buffer = '';
      hasEverHadData = false;
      if (shuttingDown) {
        console.log('[SMTC] Daemon exited (backend shutting down)');
        return;
      }
      // Exit limpo (code 0) sem nunca emitir dados = self-heal do manager
      // stale → o próximo spawn desliga o self-heal (evita loop com Spotify
      // fechado). Se emitiu dados, reabilita.
      lastDaemonExitedWithNoData = code === 0 && !hadData;
      // NÃO limpa latestData — preserva o último dado para evitar
      // que o poll loop perca a faixa atual durante o restart
      console.warn(`[SMTC] Daemon exited (code ${code}). Restarting...`);
      // Self-heal (code 0) é esperado — não reporta como erro no dashboard
      if (code !== 0 && _onError) _onError('daemon-exit', `code ${code}`);
      restartAttempts++;

      if (restartAttempts <= MAX_RESTART_ATTEMPTS) {
        const delay = Math.min(2000 * restartAttempts, 10000);
        setTimeout(() => {
          if (shuttingDown) return;
          if (!daemonAlive) startDaemon();
        }, delay);
      } else {
        ui.smtcError('SMTC daemon stopped after too many restarts');
        console.warn('[SMTC] Daemon: too many restarts, giving up.');
        restartAttempts = 0;
      }
    });

    daemonProcess.on('error', (err) => {
      daemonAlive = false;
      daemonProcess = null;
      if (shuttingDown) {
        console.log('[SMTC] Daemon error during shutdown:', err.message);
        return;
      }
      // Erro ENOENT = executável não encontrado (não compilado)
      if (err.code === 'ENOENT') {
        const msg = `C# daemon not found at ${daemonPath}. Run 'npm run build:daemon' first.`;
        console.error('[SMTC] ' + msg);
        ui.smtcError(msg);
        if (_onError) _onError('daemon-not-found', msg);
        restartAttempts = 0;
        return;
      }
      console.warn(`[SMTC] Daemon error: ${err.message}. Restarting...`);
      if (_onError) _onError('daemon-error', err.message);
      restartAttempts++;

      if (restartAttempts <= MAX_RESTART_ATTEMPTS) {
        const delay = Math.min(3000 * restartAttempts, 15000);
        setTimeout(() => {
          if (shuttingDown) return;
          if (!daemonAlive) startDaemon();
        }, delay);
      } else {
        ui.smtcError('SMTC daemon stopped after too many restarts');
        console.warn('[SMTC] Daemon: too many errors, giving up.');
        restartAttempts = 0;
      }
    });

    daemonAlive = true;
    restartAttempts = 0;
  } catch (err) {
    daemonAlive = false;
    daemonProcess = null;
    ui.smtcError(`Failed to start SMTC daemon: ${err.message}`);
  }
}

/**
 * Para o processo do daemon C#.
 */
function stopDaemon() {
  if (daemonProcess && daemonAlive) {
    try {
      daemonProcess.kill();
    } catch {}
  }
  daemonProcess = null;
  daemonAlive = false;
  latestData = null;
  lastValidData = null;
  buffer = '';
  lastDaemonOutputMs = 0;
}

/**
 * Encerramento do backend: bloqueia novos spawns/restarts e mata o filho.
 * Chamado pelos handlers de exit/SIGINT/SIGTERM — o 'exit' também roda quando
 * o processo sai via process.exit(), então nenhum daemon sobrevive a um
 * encerramento normal do backend.
 */
function shutdown() {
  shuttingDown = true;
  stopDaemon();
}

// ── Helpers ──────────────────────────────────────────────────────────────────
function makeTrackId(title, artist) {
  return `${title}::${artist}`.toLowerCase().replace(/\s+/g, '-');
}

// ── Public API ────────────────────────────────────────────────────────────────

/**
 * Aguarda até que o daemon produza o primeiro dado.
 */
async function waitForInitialData() {
  if (hasEverHadData && latestData) return;
  if (!startupPromise) {
    startupPromise = new Promise((resolve) => {
      resolveStartup = resolve;
      // Timeout de segurança
      setTimeout(() => {
        if (resolveStartup) {
          resolveStartup();
          resolveStartup = null;
          startupPromise = null;
        }
      }, STARTUP_WAIT_MS);
    });
  }
  await startupPromise;
}

/**
 * Janela de graça em ms: após receber um "não tocando" do daemon,
 * continua usando `lastValidData` por até este tempo.
 * Absorve crashes/restarts do daemon sem perder a faixa.
 */
// ── Janela de tolerância a falhas do daemon ──────────────────────────
// Aumentado de 5s para 30s para absorver:
//   - Erros fatais do daemon (reconexão em 5s)
//   - Exceções consecutivas no loop interno (vários ciclos de 1.5s)
//   - Restart do processo (backoff exponencial até 10s)
//   - Queda temporária do SMTC/Spotify
// Enquanto lastValidData estiver dentro desta janela, o programa
// CONTINUA mostrando a faixa normalmente, sem interrupção.
const FALLBACK_GRACE_MS = 30000;

/**
 * Seleciona a melhor fonte de dados entre `latestData` e `lastValidData`.
 *
 * Estratégia:
 * 1. Se `latestData` tem dados válidos (tocando, com título), usa latestData.
 * 2. Se `latestData` diz "não tocando" mas `lastValidData` ainda tem faixa
 *    válida e recente (< FALLBACK_GRACE_MS), usa `lastValidData` com progresso
 *    estimado. Absorve crashes/restarts do daemon.
 * 3. Caso contrário, retorna o que estiver disponível (ou null).
 */
function selectBestData() {
  // ── Caso 1: daemon tem dados frescos e tocando ──────────────────────────
  if (latestData && latestData.isPlaying && latestData.title) {
    if (latestData !== lastValidData && lastValidData) {
      console.log(`[SMTC] Daemon recovered (valid data after ${((Date.now() - (lastValidData.lastUpdatedMs || Date.now())) / 1000).toFixed(1)}s gap)`);
    }
    return latestData;
  }

  // ── Caso 2: daemon EXPLICITAMENTE diz "não tocando" (pausa real) ───────
  // NÃO usar lastValidData como fallback! Isso permite que o poll loop
  // entre na histerese curta (3s) e limpe o status rapidamente.
  // O fallback só deve ser usado para crash/glitch do daemon (caso 3).
  if (latestData && latestData.isPlaying === false && !latestData.psFatal && !latestData.psError) {
    return latestData;  // {isPlaying: false}
  }

  // ── Caso 3: daemon offline/erro — usar lastValidData como fallback ─────
  // Absorve crashes e restarts do daemon. Só ativo se o daemon NÃO está
  // dizendo explicitamente "não tocando" (caso 2 já tratou isso).
  if (lastValidData && lastValidData.isPlaying && lastValidData.title) {
    const elapsedSinceValid = Date.now() - (lastValidData.lastUpdatedMs || Date.now());
    if (elapsedSinceValid < FALLBACK_GRACE_MS) {
      return lastValidData;
    }
    console.warn(`[SMTC] Grace period expired (${(elapsedSinceValid / 1000).toFixed(1)}s since last valid data). Track will be cleared.`);
  }

  // ── Caso 4: sem dados úteis ────────────────────────────────────────────
  return latestData?.isPlaying ? latestData : null;
}

/**
 * Retorna true se o daemon ESTÁ ATIVO e está explicitamente reportando "não tocando"
 * (isPlaying=false, sem erros). Diferente de "daemon offline/sem dados/erro".
 * Usado pelo poll() em index.js para decidir histerese curta (pausa real) vs longa (glitch).
 */
export function isExplicitlyNotPlaying() {
  return latestData !== null
    && latestData.isPlaying === false
    && !latestData.psFatal
    && !latestData.psError;
}

/**
 * Retorna a faixa atual obtida pelo daemon C# SMTC.
 * O daemon é iniciado automaticamente na primeira chamada.
 * Se for a primeira chamada, aguarda até 5s pelo primeiro dado.
 */
export async function getCurrentTrack() {
  const isFirstStart = !daemonAlive || !daemonProcess;

  if (isFirstStart) {
    await startDaemon();
    await waitForInitialData();
  }

  // ── Seleção inteligente de dados ──────────────────────────────────────────
  const data = selectBestData();
  if (!data) return null;

  // ── Verifica erros do daemon ─────────────────────────────────────────────
  if (data.psFatal) {
    console.error('[SMTC] Fatal error from daemon:', data.psFatal);
    return { isPlaying: false, error: data.psFatal };
  }
  if (data.psError) {
    console.warn('[SMTC] Non-fatal error from daemon:', data.psError);
  }

  // ── FILTRO: ACEITA SPOTIFY (vários UserModelId possíveis) ────────────────
  const appId = (data.sourceApp || '').toLowerCase();
  const isSpotify = appId.includes('spotify') || appId.includes('spotifyab') || appId.includes('spotify.music');

  if (!isSpotify) {
    return { isPlaying: false };
  }

  if (!data.isPlaying) {
    // ── PAUSA REAL: preserva a faixa (título/artista) ──────────────
    // Assim a UI continua mostrando a música como PAUSADA em vez de
    // voltar para "No track playing".
    if (!data.title) return { isPlaying: false }; // sessão encerrada, sem faixa
    // Guarda de liveness do daemon: se ele não emite há >15s (morreu ou
    // está em restart — backoff máximo é 10s + ~1s da primeira emissão),
    // cai no fluxo de glitch em vez de segurar a pausa para sempre.
    // Usa lastDaemonOutputMs (que atualiza a cada ~1s de emissão) e NÃO
    // data.lastUpdatedMs do SMTC — este congela durante a pausa legítima.
    if (Date.now() - lastDaemonOutputMs > 15000) return { isPlaying: false };
    return {
      isPlaying: false,
      trackId: makeTrackId(data.title, data.artist),
      trackName: data.title,
      artistName: data.artist,
      albumName: data.albumTitle || '',
      progressMs: data.positionMs || 0,
      durationMs: data.durationMs || 0,
      albumArtUrl: '',
      platform: 'spotify',
    };
  }
  if (!data.title) return null;

  const smtcAge = Math.max(0, Date.now() - (data.lastUpdatedMs || Date.now()));
  const rawProgress = data.isPlaying
    ? data.positionMs + smtcAge
    : data.positionMs;
  const progressMs = Math.min(Math.max(0, rawProgress), data.durationMs || 0);

  return {
    isPlaying: !!data.isPlaying,
    trackId: makeTrackId(data.title, data.artist),
    trackName: data.title,
    artistName: data.artist,
    albumName: data.albumTitle || '',
    progressMs,
    durationMs: data.durationMs || 0,
    albumArtUrl: '',
    platform: 'spotify',
  };
}

/**
 * Envia comando de seek para o SMTC do Windows via daemon C# compilado.
 * Executa SmtcDaemon.exe seek <positionMs> e espera OK/FAIL no stdout.
 * @param {number} positionMs Posição em milissegundos
 */
export async function seekTrack(positionMs) {
  const daemonPath = resolveDaemonPath();

  return new Promise((resolve, reject) => {
    const child = spawn(daemonPath, ['seek', String(Math.round(positionMs))], {
      windowsHide: true,
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    let stdout = '';
    let stderr = '';

    child.stdout.on('data', (chunk) => { stdout += chunk.toString(); });
    child.stderr.on('data', (chunk) => { stderr += chunk.toString(); });

    child.on('close', (code) => {
      if (code !== 0) {
        reject(new Error(stderr.trim() || `exit code ${code}`));
      } else if (stdout.includes('OK')) {
        resolve();
      } else if (stdout.includes('FAIL')) {
        // Seek rejeitado (ex: sem sessão ativa) — não é erro fatal
        resolve();
      } else {
        reject(new Error(stdout.trim() || 'Unknown error'));
      }
    });

    child.on('error', (err) => {
      reject(err);
    });

    // Timeout de segurança (5s)
    setTimeout(() => {
      try { child.kill(); } catch {}
      reject(new Error('Seek timeout'));
    }, 5000);
  });
}

/**
 * Reinicia o daemon C#.
 */
export function resetDaemon() {
  stopDaemon();
  latestData = null;
  buffer = '';
}

// Limpeza na saída do processo
// 'exit' roda até em process.exit(); os signals também matam o filho e
// bloqueiam novos spawns. O watchdog do daemon (SMTC_PARENT_PID) cobre o que
// o Node não vê — taskkill /F, crash nativo, kill -9: nesses casos o daemon
// se auto-encerra em ≤1s em vez de virar órfão monopolizando o SMTC.
process.on('exit', () => shutdown());
process.on('SIGINT', () => { shutdown(); process.exit(0); });
process.on('SIGTERM', () => { shutdown(); process.exit(0); });
