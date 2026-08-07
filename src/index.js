import 'dotenv/config';
import { spawn } from 'child_process';
import { getCurrentTrack, isExplicitlyNotPlaying, setSmtcErrorHandler, seekTrack } from './spotify.js';
import { getLyrics } from './lyrics.js';
import { getAlbumArt } from './album-art.js';
import { getTrackCredits } from './credits.js';
import { setCustomStatus, clearCustomStatus, onDiscordLatencyChange, getEstimatedLatencyMs } from './status.js';
import { LyricScheduler } from './scheduler.js';
import { getToken, getSetting, saveSetting } from './config.js';
import { acquireLock, releaseLock } from './lock.js';
import { DashboardServer } from './server.js';
import { attachSetupRoutes, resolveDiscordAccount } from './setup.js';
import * as ui from './ui.js';

// Detecta se está rodando dentro do Electron
const IN_ELECTRON = process.env.ELECTRON_RUN === '1';

const POLL_INTERVAL_PLAYING_MS = 1000;
const POLL_INTERVAL_IDLE_MS = 1000;
// Modo econômico: polia menos agressivamente para reduzir CPU/RAM
const POLL_INTERVAL_PLAYING_ECO_MS = 2000;
const POLL_INTERVAL_IDLE_ECO_MS = 5000;
// Polling adaptativo de idle: se nenhuma música tocar por IDLE_BACKOFF_AFTER_MS,
// o poll desacelera MESMO sem modo econômico — o app ocioso não precisa checar
// a cada 1s. Volta ao ritmo rápido assim que a reprodução é detectada.
// Cap conservador (3s/5s): o pior caso de detecção de uma nova faixa após
// ocioso fica ≤5s — sem reincidir na reclamação anterior de reconhecimento
// lento — enquanto ainda economiza 3–5x de CPU vs. o idle de 1s.
const IDLE_BACKOFF_AFTER_MS = 5 * 60 * 1000;     // 5 minutos sem música
const POLL_INTERVAL_IDLE_BACKOFF_MS = 3000;       // normal: 3s
const POLL_INTERVAL_IDLE_BACKOFF_ECO_MS = 5000;   // eco: 5s (igual ao eco idle)
// Tick do scheduler de letras (50ms normal / 100ms econômico)
const SCHEDULER_TICK_MS = 50;
const SCHEDULER_TICK_ECO_MS = 100;
// Limiar de detecção de seek. O SMTC quantiza a posição em ~1s: se o valor
// reportado "congelar" por 1-2 polls (quantização), a diferença vs. a
// interpolação local chegaria a ~1s e dispararia um seek FALSO, reiniciando
// o scheduler e atrasando as letras. 1500ms distingue quantização de um
// pulo real do usuário (arrastar a barra).
const SEEK_THRESHOLD_MS = 1500;
const TRACK_CHANGE_DEBOUNCE_MS = 400;

/** Modo econômico: true reduz polling e ticks para economizar CPU/RAM. */
let ecoMode = getSetting('ecoMode', false) === true;

// ── Histerese temporal ───────────────────────────────────────────────
// DUAS histereses diferentes:
//
// 1. PAUSA REAL (daemon online, isPlaying=false): Usuário apertou pause.
//    O status do Discord é limpo quase imediatamente (PAUSE_STATUS_CLEAR_MS),
//    mas a faixa continua visível na UI como PAUSADA.
//
// 2. GLITCH / DAEMON OFFLINE (15s): O daemon caiu, está restartando, ou
//    selectBestData() retornou null. Não limpa a faixa imediatamente
//    para absorver restarts e erros transientes.
//
// A distinção é feita por isExplicitlyNotPlaying() em spotify.js.
let notPlayingSince = null;
const PAUSE_TIMEOUT_MS = 3000;
// Pausa REAL confirmada (daemon online, isPlaying=false, faixa conhecida):
// limpa o status do Discord quase imediatamente. Antes esperava
// PAUSE_TIMEOUT_MS (3s) — o status ficava "Tocando…" no Discord por vários
// segundos após pausar. A graça curta (500ms) só absorve pause/resume rápido.
const PAUSE_STATUS_CLEAR_MS = 500;
const GLITCH_TIMEOUT_MS = 15000;

/** Timestamp (Date.now) da última vez que uma faixa esteve de fato tocando. */
let lastPlaybackAt = Date.now();
/** Flag: true enquanto o poll está no modo backoff (transição logada 1x). */
let idleBackoffActive = false;

/** Flag: true quando o scheduler está pausado por inatividade */
let schedulerPaused = false;
/** Flag: o status do Discord já foi limpo nesta sessão de pausa (evita repetição) */
let pauseStatusCleared = false;
/** trackId da última faixa pausada transmitida — evita broadcast repetido a cada poll */
let lastPausedBroadcastTrackId = null;



// ── Emojis do status (personalizáveis) ───────────────────────────────
// Emotes customizados do usuário: podem ser emojis unicode (🎵) OU emotes
// do Discord, ex: <:name:id> (estático) e <a:name:id> (animado — Nitro).
// null/'' = usa a lista aleatória padrão abaixo.
let customTrackEmoji = getSetting('trackEmoji', null);
let customLyricsEmoji = getSetting('lyricsEmoji', null);

// Array de emojis relacionados a música para o status da faixa
const TRACK_EMOJIS = ['🎵', '🎶', '🎼', '🎧', '🎹', '🎸', '🎷', '🎺', '🎻', '🥁', '📻'];
// Array de emojis para quando a letra está ativa
const LYRICS_EMOJIS = ['🎤', '🎵', '🎶', '🎼', '🎧'];

/** Escolhe um elemento aleatório de um array */
function pickRandom(arr) {
  return arr[Math.floor(Math.random() * arr.length)];
}

/**
 * Callback do LyricScheduler — disparado a cada mudança de linha sincronizada.
 * Atualiza o status do Discord com a letra atual e notifica a dashboard
 * (evento 'lyric-line', que move o destaque de linha no frontend).
 * (Esta função era referenciada sem declaração, causando ReferenceError no
 * primeiro agendamento de letras — junto com _lastTrackName/_lastArtistName.)
 *
 * @param {{timeMs: number, text: string, index: number}} line
 */
function onLineChange(line) {
  if (!line || typeof line.index !== 'number') return;
  currentLyricIndex = line.index;
  firstLyricAfterTrackChange = false;
  if (line.text) {
    try {
      setCustomStatus(line.text, getLyricsEmoji());
    } catch (err) {
      console.error('[Line] Status error:', err.message);
    }
    ui.lyricLine(line.text);
  }
  server.broadcast('lyric-line', { index: line.index, text: line.text || '' });
}

/** Retorna o emoji da faixa: customizado se configurado, senão aleatório */
function getTrackEmoji() { return customTrackEmoji || pickRandom(TRACK_EMOJIS); }
/** Retorna o emoji das letras: customizado se configurado, senão aleatório */
function getLyricsEmoji() { return customLyricsEmoji || pickRandom(LYRICS_EMOJIS); }

// ── State ─────────────────────────────────────────────────────────────────────

let currentTrackId = null;
let currentPlatform = 'unknown';
let currentAlbumArtUrl = null;
let currentDurationMs = 0;
let scheduler = null;
let lastProgressMs = 0;
let lastPollTime = 0;
let pollTimer = null;
let loadingLyrics = false;

let pendingTrackId = null;
let pendingTrack = null;
let pendingPlatform = null;
let debounceTimer = null;

let currentLyrics = [];
let currentLyricIndex = -1;
// Créditos da faixa atual (artistas completos/feats, produtores, escritores).
// Buscado via Genius API em background — null até resolver (ou se não achar).
let currentCredits = null;
let creditsVersion = 0;
let discordAccount = null;

/** Última faixa/artista exibidos — usados pelo state collector quando não há track pendente.
 *  (Estas variáveis eram usadas sem declaração, o que lançava ReferenceError em ESM
 *  e travava TODOS os broadcasts de estado: capa, letras e status nunca chegavam à UI.) */
let _lastTrackName = null;
let _lastArtistName = null;
/** trackId da última faixa pausada para a qual capa/letras já foram buscadas em background */
let pausedDataTrackId = null;

/** Versão incremental — detecta mudança de letras mesmo quando mesmo track */
let lyricsVersion = 0;
/** Flag: true = a primeira letra da faixa ainda não disparou o status,
 *  mantendo "🎵 TrackName — Artist" visível no Discord */
let firstLyricAfterTrackChange = true;

/** true = letras sincronizadas (LyricScheduler ativo), false = texto rolando (Genius) */
let lyricsSynced = true;
let appReady = false;
let discordStatus = 'checking';
let smtcStatus = 'checking';
let lyricsSourceStatus = 'checking';
let lyricsState = 'idle';

async function refreshDiscordAccount() {
  const token = getToken();
  if (!token) {
    discordAccount = null;
    return;
  }

  try {
    const account = await resolveDiscordAccount(token);
    if (account) {
      discordAccount = account;
      server.broadcastState();
    } else {
      discordAccount = null;
    }
  } catch {
    discordAccount = null;
  }
}

// ── Configurações de sincronia (persistidas entre tracks) ────────────────
let currentSyncLeadMs = 200;
/** true = adaptive sync ativo (ajusta o lead pela latência da API do Discord).
 *  Ajuste manual do slider desativa; reativado pela UI (settings.adaptiveSync). */
let adaptiveSyncEnabled = true;

// ── Broadcast throttle ────────────────────────────────────────────────────
// Evita enviar SSE a cada 1.5s quando apenas o progressMs mudou.
// O frontend interpola a barra de progresso localmente via RAF.
let lastBroadcastKey = '';
let broadcastHeartbeatCount = 0;
let broadcastHeartbeatSeq = 0;
const BROADCAST_HEARTBEAT_EVERY = 3; // Força broadcast a cada ~4.5s mesmo sem mudanças

/**
 * Cache de string serializada — evita dupla serialização entre shouldBroadcast e broadcastState.
 */
let lastStateSerialized = null;

/**
 * Compara o snapshot atual com o último broadcast e decide se vale a pena emitir.
 * @param {object} snapshot Estado completo (com progressMs)
 * @returns {string|false} String serializada se deve broadcast, ou false se não mudou
 */
function shouldBroadcast(snapshot) {
  // Serializa APENAS os campos que importam para comparar
  // (exclui progressMs, lyrics — que mudam constantemente)
  const key = JSON.stringify([
    snapshot.trackName,
    snapshot.artistName,
    snapshot.albumArtUrl,
    snapshot.creditsVersion,
    snapshot.lyricsVersion,
    snapshot.durationMs,
    snapshot.isPlaying,
    snapshot.discordStatus,
    snapshot.smtcStatus,
    snapshot.lyricsSourceStatus,
    snapshot.lyricsState,
    snapshot.lyricsSynced,
    snapshot.currentLyricIndex,
    snapshot.lyricsCount,
    snapshot.syncLeadMs,
    Math.floor(broadcastHeartbeatCount / BROADCAST_HEARTBEAT_EVERY), // heartbeat seq incluído
  ]);

  if (key !== lastBroadcastKey) {
    lastBroadcastKey = key;
    broadcastHeartbeatCount = 0;
    // Serializa o snapshot COMPLETO para o broadcast (incluindo progressMs, lyrics, etc.)
    lastStateSerialized = JSON.stringify(snapshot);
    return lastStateSerialized;
  }

  broadcastHeartbeatCount++;
  // Força um heartbeat a cada N polls para o frontend corrigir drift
  if (broadcastHeartbeatCount >= BROADCAST_HEARTBEAT_EVERY) {
    broadcastHeartbeatCount = 0;
    broadcastHeartbeatSeq++;
    // Reserializa porque progressMs mudou
    lastStateSerialized = JSON.stringify(snapshot);
    return lastStateSerialized;
  }

  return false;
}

// ── Server ────────────────────────────────────────────────────────────────────

const server = new DashboardServer();

server.setStateCollector(() => ({
  platform: currentPlatform,
  trackName: pendingTrack?.trackName || _lastTrackName || null,
  artistName: pendingTrack?.artistName || _lastArtistName || null,
  albumArtUrl: currentAlbumArtUrl,
  // Créditos (feats/produtores/escritores) da faixa atual — exibidos junto à
  // barra de progresso. creditsVersion permite o frontend detectar a chegada
  // assíncrona mesmo com o resto do estado igual.
  credits: currentCredits,
  creditsVersion,
  lyricsVersion,
  progressMs: lastProgressMs,
  // currentDurationMs é mantido pelo poll para a faixa ATUAL (tocando OU
  // pausada). pendingTrack?.durationMs só existe na janela de debounce
  // (~400ms) e vira 0 depois — usar ele congelava a barra de progresso
  // (durationMs 0 → o loop RAF da UI morre e o watchdog não reinicia).
  durationMs: currentDurationMs || 0,
  // isPlaying reflete o estado REAL: false se estiver em histerese (pausa/glitch)
  isPlaying: (currentTrackId !== null || pendingTrack !== null) && notPlayingSince === null,
  discordStatus,
  smtcStatus,
  lyricsSourceStatus,
  lyricsState,
  lyrics: currentLyrics,
  lyricsSynced,
  currentLyricIndex,
  lyricsCount: currentLyrics.length,
  lyricsStatusText: null,
  syncLeadMs: currentSyncLeadMs,
  // Latência média estimada da API do Discord (EWMA) — exibida ao vivo na UI
  discordLatencyMs: getEstimatedLatencyMs(),
  // Estado do adaptive sync — a UI mostra "Ativo/Manual" e o botão de reativar
  adaptiveSyncEnabled,
  trackEmoji: customTrackEmoji,
  lyricsEmoji: customLyricsEmoji,
  appReady,
  tokenConfigured: !!getToken(),
  discordAccountName: discordAccount?.username || null,
  discordAccountTag: discordAccount?.tag || null,
  discordAccountAvatarUrl: discordAccount?.avatarUrl || null,
}));

// ── Poll loop ─────────────────────────────────────────────────────────────────

async function poll() {
  const tickStart = performance.now();

  try {
    const track = await getCurrentTrack();

    if (!track || !track.isPlaying) {
      // ── Pausa real vs. glitch vs. sessão encerrada ─────────────────────
      //   PAUSA REAL (daemon online, isPlaying=false + faixa conhecida):
      //       → a música PERMANECE na UI como PAUSADA (badge PAUSED).
      //         Só o status do Discord é limpo após PAUSE_TIMEOUT_MS.
      //   SEM FAIXA (título vazio) ou GLITCH (daemon offline/erro):
      //       → histerese original (3s pausa explícita / 15s glitch) e limpeza.
      const explicitPause = track && !track.isPlaying && isExplicitlyNotPlaying();
      // Faixa ainda identificada (título presente) → pausa real com música conhecida
      const pausedWithTrack = explicitPause && !!track.trackName && !!track.trackId;

      if (currentTrackId !== null) {
        // Pausa o scheduler uma vez — letras congelam na posição atual
        if (scheduler && !schedulerPaused) {
          scheduler.pause();
          schedulerPaused = true;
          server.broadcastLog('SCHEDULER', explicitPause
            ? 'Paused (user paused)'
            : 'Paused (no data — waiting for daemon)');
        }

        if (pausedWithTrack) {
          // ── PAUSA REAL com faixa conhecida: mantém a música na UI ──
          // isPlaying=false, mas faixa, capa e letras continuam visíveis.
          // Congela o progresso na posição pausada ANTES do broadcast —
          // o frontend congela a barra/anel na posição real, sem salto para
          // trás (collectState lê lastProgressMs no momento do broadcast).
          lastProgressMs = track.progressMs || 0;
          // Mantém a duração da faixa pausada no estado (durationMs: 0
          // congelava a barra de progresso na UI).
          currentDurationMs = track.durationMs || currentDurationMs;
          const pausedTrackChanged = track.trackId !== currentTrackId;
          if (pausedTrackChanged) {
            // Usuário selecionou OUTRA música enquanto pausado: atualiza a
            // exibição, mas NÃO mexe em currentTrackId — ao retomar, o
            // fluxo completo de track-change recarrega letras e capa.
            _lastTrackName = track.trackName;
            _lastArtistName = track.artistName;
            currentLyrics = [];
            currentLyricIndex = -1;
            lyricsState = 'idle';
            currentAlbumArtUrl = null;
          // Desmonta o scheduler stale (letras da faixa ANTIGA) — se
          // ficar vivo, o resume poderia disparar uma letra errada na
          // faixa nova antes do applyTrackChange rodar.
          if (scheduler) {
            scheduler.stop();
            scheduler = null;
          }
          schedulerPaused = false;

          // Busca capa + letras da faixa pausada em background (uma vez por
          // faixa) — a UI fica completa (arte + letras) mesmo com o Spotify
          // pausado, em vez de placeholder sem letras.
          if (pausedDataTrackId !== track.trackId) {
            pausedDataTrackId = track.trackId;
            loadPausedTrackData(track);
          }
        }

          // ── Broadcast (independe do clear do Discord) ──────────────
          // currentTrackId NÃO muda na troca de faixa pausada (de propósito),
          // então pausedTrackChanged ficaria true em todo poll — por isso o
          // broadcast é gateado pelo trackId da última faixa transmitida, e
          // o clear do status do Discord é um `if` independente (senão o
          // else-if encadeado passaria fome e nunca limparia o status).
          const needBroadcast = notPlayingSince === null
            || track.trackId !== lastPausedBroadcastTrackId;
          if (needBroadcast) {
            lastPausedBroadcastTrackId = track.trackId;
            if (notPlayingSince === null) notPlayingSince = Date.now();
            server.broadcastState();
          }

          if (!pauseStatusCleared && notPlayingSince !== null
              && Date.now() - notPlayingSince >= PAUSE_STATUS_CLEAR_MS) {
            // Pausa confirmada: limpa o status do Discord quase na hora (não
            // fica "ouvindo" algo pausado) — mas a UI segue mostrando a faixa.
            pauseStatusCleared = true;
            clearCustomStatus();
            server.broadcastLog('STATUS', 'Discord status cleared (paused)');
          }

          lastPollTime = tickStart;
          return;
        }

        // ── Sessão encerrada ou glitch: histerese original ────────────
        const timeoutMs = explicitPause ? PAUSE_TIMEOUT_MS : GLITCH_TIMEOUT_MS;
        if (notPlayingSince === null) {
          notPlayingSince = Date.now();
          // Broadcast imediato: frontend mostra pausa
          server.broadcastState();
        }

        if (Date.now() - notPlayingSince < timeoutMs) {
          // Ainda dentro do timeout: preserva a faixa mas com isPlaying=false
          // O frontend já recebeu o broadcast com isPlaying=false
          lastPollTime = tickStart;
          return;
        }

        // Timeout excedido: realmente parou de tocar
        notPlayingSince = null;
        pauseStatusCleared = false;
        schedulerPaused = false;
        smtcStatus = 'checking';
        ui.playbackStopped();
        loadingLyrics = false;
        scheduler = null;
        currentTrackId = null;
        currentPlatform = 'unknown';
        // Limpa duração/capa também — senão o estado carregaria a duração e a
        // arte da faixa anterior após a sessão encerrar.
        currentDurationMs = 0;
        currentAlbumArtUrl = null;
        _lastTrackName = null;
        _lastArtistName = null;
        pausedDataTrackId = null;
        currentLyrics = [];
        currentLyricIndex = -1;
        lyricsState = 'idle';
        currentCredits = null;
        creditsVersion++;
        clearCustomStatus();
        ui.statusBar(!!getToken(), currentPlatform);
        server.broadcastState();
      } else {
        notPlayingSince = null;
        pauseStatusCleared = false;
        smtcStatus = 'checking';
      }

      lastPollTime = tickStart;
      return;
    }

    // Resetou o timer de histerese porque está tocando
    notPlayingSince = null;
    // Atualiza o relógio de reprodução — usado pelo polling adaptativo de idle
    lastPlaybackAt = Date.now();
    // Se a pausa limpou o status do Discord, restaura "Track — Artist"
    // (a próxima linha de letra disparada substitui pela letra correta)
    if (pauseStatusCleared) {
      pauseStatusCleared = false;
      setCustomStatus(track.trackName + ' — ' + track.artistName, getTrackEmoji());
    }
    // Se o scheduler estava pausado, retoma
    if (schedulerPaused && scheduler) {
      schedulerPaused = false;
      scheduler.resume(track.progressMs);
      server.broadcastLog('SCHEDULER', 'Resumed after pause');
    }
    smtcStatus = 'connected';

    if (track.trackId !== currentTrackId) {
      currentDurationMs = track.durationMs || 0;
      // Zera a capa da faixa ANTERIOR já no PRIMEIRO broadcast da faixa nova
      // (antes do applyTrackChange rodar) — senão o estado sai com o nome da
      // música nova mas a capa velha, e a UI mostra/blur fica na arte antiga.
      currentAlbumArtUrl = null;
      if (pendingTrackId === null) {
        pendingTrackId = track.trackId;
        pendingTrack = track;
        pendingPlatform = track.platform || 'unknown';
        _lastTrackName = track.trackName;
        _lastArtistName = track.artistName;

        debounceTimer = setTimeout(async () => {
          debounceTimer = null;
          if (pendingTrackId === track.trackId && pendingTrackId !== currentTrackId) {
            await applyTrackChange(pendingTrack, pendingPlatform, tickStart);
          }
          pendingTrackId = null;
          pendingTrack = null;
          pendingPlatform = null;
        }, TRACK_CHANGE_DEBOUNCE_MS);
      } else if (pendingTrackId !== track.trackId) {
        pendingTrackId = track.trackId;
        pendingTrack = track;
        pendingPlatform = track.platform || 'unknown';
        _lastTrackName = track.trackName;
        _lastArtistName = track.artistName;
      }

      lastProgressMs = track.progressMs || 0;
      lastPollTime = tickStart;
      server.broadcastState();
      return;
    }

    if (debounceTimer && pendingTrackId === track.trackId) {
      clearTimeout(debounceTimer);
      debounceTimer = null;
      pendingTrackId = null;
      pendingTrack = null;
      pendingPlatform = null;
    }

    if (scheduler && !loadingLyrics) {
      // Calibra o scheduler com o progresso real do SMTC (malha fechada)
      scheduler.updateProgress(track.progressMs);

      if (lastPollTime > 0) {
        const elapsed = tickStart - lastPollTime;
        const expectedProgress = lastProgressMs + elapsed;
        const drift = Math.abs(track.progressMs - expectedProgress);

        if (drift > SEEK_THRESHOLD_MS) {
          ui.seekDetected(drift / 1000);
          scheduler.restart(track.progressMs);
        }
      }
    }

    lastProgressMs = track.progressMs || 0;
    lastPollTime = tickStart;

    // Broadcast apenas se mudou algo relevante (progressMs sozinho não dispara).
    // Sem clientes WebSocket conectados, nem coleta nem serializa o estado
    // (que inclui o array de letras — o mais caro) — economia de CPU/memória.
    if (server.hasClients()) {
      const snapshot = server.collectState();
      const serialized = shouldBroadcast(snapshot);
      if (serialized !== false) {
        server.broadcast('state', serialized);
      }
    } else {
      // Ninguém assistindo: mantém o cache de comparação limpo para quando
      // um cliente conectar, o estado inicial é enviado no handshake mesmo.
      lastBroadcastKey = null;
      lastStateSerialized = null;
      broadcastHeartbeatCount = 0;
    }
  } catch (err) {
    console.error('[Poll] Error:', err.message);
    smtcStatus = 'disconnected';
    lastPollTime = tickStart;
    notPlayingSince = null;
  } finally {
    // ── ÚNICO ponto de agendamento do próximo poll ──
    // Todos os early returns acima (return;) delegam o timer para cá.
    // Isso elimina o bug de timers duplicados que causava polling excessivo.
    const elapsed = performance.now() - tickStart;

    // ── Polling adaptativo de idle ────────────────────────────────────
    // Se nada toca há IDLE_BACKOFF_AFTER_MS (Spotify fechado, app ocioso,
    // faixa pausada há muito tempo), desacelera o poll — mesmo sem eco mode.
    const useBackoff = (Date.now() - lastPlaybackAt) >= IDLE_BACKOFF_AFTER_MS;

    if (useBackoff && !idleBackoffActive) {
      idleBackoffActive = true;
      ui.idleBackoff(ecoMode ? POLL_INTERVAL_IDLE_BACKOFF_ECO_MS : POLL_INTERVAL_IDLE_BACKOFF_MS);
    } else if (!useBackoff && idleBackoffActive) {
      idleBackoffActive = false;
      ui.idleBackoffResumed();
    }

    let interval;
    if (useBackoff) {
      // Nada tocando há muito tempo: ritmo lento para economizar CPU/bateria
      interval = ecoMode ? POLL_INTERVAL_IDLE_BACKOFF_ECO_MS : POLL_INTERVAL_IDLE_BACKOFF_MS;
    } else if (currentTrackId) {
      // Tocando (ou pausa recente): ritmo rápido
      interval = ecoMode ? POLL_INTERVAL_PLAYING_ECO_MS : POLL_INTERVAL_PLAYING_MS;
    } else {
      // Idle recente: ritmo normal de idle
      interval = ecoMode ? POLL_INTERVAL_IDLE_ECO_MS : POLL_INTERVAL_IDLE_MS;
    }
    const delay = Math.max(0, interval - elapsed);
    pollTimer = setTimeout(poll, Math.min(delay, 2147483647));
  }
}

async function applyTrackChange(track, platform, detectedAtTickStart) {
  ui.trackChange(track.trackName, track.artistName, platform);

  currentTrackId = track.trackId;
  currentPlatform = platform;
  _lastTrackName = track.trackName;
  _lastArtistName = track.artistName;

  if (scheduler) {
    scheduler.stop();
    scheduler = null;
  }

  loadingLyrics = true;
  lyricsState = 'loading';
  // NÃO limpa currentLyrics nem currentLyricIndex — preserva as letras
  // da faixa anterior para transição suave (o frontend mostra dimmed)
  // Só limpa quando as novas letras chegarem (ou der erro)
  lyricsSynced = true;
  currentAlbumArtUrl = null;
  // Limpa créditos da faixa anterior e dispara a busca dos novos em background
  currentCredits = null;
  creditsVersion++;
  // Zera o marcador de faixa pausada: se uma faixa pausada A estava em fetch
  // (pausedDataTrackId = A) e agora uma faixa B começa a tocar, sem este reset
  // o guard de loadTrackCredits (pausedDataTrackId !== track.trackId) deixaria
  // os créditos stale de A sobrescreverem os de B ao resolver.
  pausedDataTrackId = null;
  loadTrackCredits(track);
  firstLyricAfterTrackChange = true;
  setCustomStatus(track.trackName + ' — ' + track.artistName, getTrackEmoji());
  ui.statusBar(!!getToken(), currentPlatform, track.trackName, track.artistName);
  server.broadcastState();

  try {
    const lyrics = await getLyrics(track.trackId, track.trackName, track.artistName, track.albumName, track.durationMs);

    if (track.trackId !== currentTrackId) {
      loadingLyrics = false;
      lyricsState = 'idle';
      ui.trackChange(track.trackName, track.artistName, platform);
      server.broadcastState();
      return;
    }

    loadingLyrics = false;

    if (lyrics && lyrics.lines && lyrics.lines.length > 0) {
      lyricsSynced = lyrics.synced !== false;
      lyricsVersion++;
      currentLyrics = lyrics.lines;
      currentLyricIndex = -1;

      if (lyrics.synced) {
        // Letras sincronizadas → usa LyricScheduler para disparar no tempo certo
        scheduler = new LyricScheduler(lyrics.lines, onLineChange);
        scheduler.setLeadMs(currentSyncLeadMs);
        // Tick adaptativo conforme o modo econômico
        scheduler.setTickIntervalMs(ecoMode ? SCHEDULER_TICK_ECO_MS : SCHEDULER_TICK_MS);
        lyricsState = 'active';
        // Broadcast state PRIMEIRO para o frontend ter o array de letras,
        // DEPOIS inicia o scheduler (que dispara #tick() imediatamente)
        // Assim o evento 'lyric-line' nunca chega antes do array de letras
        // Compensa o tempo decorrido desde que o progressMs foi capturado no poll
    // O track.progressMs reflete a posição no momento de detectAtTickStart.
    // Como houve debounce + fetch de letras, o progresso real já avançou.
    const elapsedSincePoll = performance.now() - (detectedAtTickStart || lastPollTime);
    const adjustedProgress = Math.min(track.progressMs + elapsedSincePoll, track.durationMs || track.progressMs);
    server.broadcastState();
    scheduler.start(adjustedProgress);
        lyricsSourceStatus = 'connected';
        server.broadcastLog('LYRICS', `Loaded ${lyrics.lines.length} synced lines for ${track.trackName}`);
      } else {
        // Letras NÃO sincronizadas (Genius) → mostra como texto rolante
        // O frontend calcula a linha atual baseado no progresso
        lyricsState = 'active';
        lyricsSourceStatus = 'connected';
        server.broadcastLog('LYRICS', `Loaded ${lyrics.lines.length} unsynced lines (Genius) for ${track.trackName}`);
      }
    } else {
      lyricsState = 'empty';
      lyricsSourceStatus = 'disconnected';
      // Só limpa letras quando a nova busca falhou (não tem lyrics mesmo)
      currentLyrics = [];
      currentLyricIndex = -1;
    }

    // Dispatch album art + preview URL fetch em paralelo e lida no then()
    // para fazer UM unico broadcast com ambos resolvidos
    const batchData = await Promise.all([
      getAlbumArt(track.trackId, track.trackName, track.artistName, track.albumName).catch(() => ({ url: null })),
    ]);

    const [{ url: artUrl }] = batchData;
    if (track.trackId === currentTrackId) {
      currentAlbumArtUrl = artUrl;
      server.broadcastState();
      if (artUrl) {
        server.broadcastLog('ALBUMART', `Loaded cover art for ${track.trackName}`);
      } else {
        server.broadcastLog('ALBUMART', `No cover art found — using gradient placeholder for ${track.trackName}`);
      }
    }
  } catch (lyricsError) {
    loadingLyrics = false;
    lyricsState = 'empty';
    lyricsSourceStatus = 'disconnected';
    currentLyrics = [];
    currentLyricIndex = -1;
    console.error('[Lyrics] Error:', lyricsError?.message ?? lyricsError);
    ui.fetchError('Lyrics', lyricsError?.message ?? 'Unknown error');
    server.broadcastState();
  }
}

// ── Dados de faixa pausada (background) ───────────────────────────────────────

/**
 * Busca capa e letras para uma faixa exibida PAUSADA (sem iniciar o scheduler).
 * Quando o usuário pausa, o fluxo normal (applyTrackChange) não roda — sem isto
 * a UI mostraria a faixa pausada apenas com placeholder e sem letras.
 * Chamado uma única vez por faixa pausada (guard via pausedDataTrackId).
 *
 * @param {object} track Dados da faixa (trackId, trackName, artistName, albumName, durationMs)
 */
async function loadPausedTrackData(track) {
  if (!track || !track.trackId || !track.trackName) return;

  try {
    const [lyrics, art] = await Promise.all([
      getLyrics(track.trackId, track.trackName, track.artistName, track.albumName, track.durationMs).catch(() => null),
      getAlbumArt(track.trackId, track.trackName, track.artistName, track.albumName).catch(() => ({ url: null })),
    ]);
    // Créditos (feats/produtores/escritores) também para faixas pausadas
    loadTrackCredits(track);

    // Guardas anti-race: descarta resultado stale se (a) a reprodução voltou
    // (applyTrackChange vira autoridade e define currentTrackId) ou (b) o
    // usuário já trocou para OUTRA faixa pausada — lastPausedBroadcastTrackId
    // aponta sempre para a faixa pausada exibida mais recente, então um fetch
    // lento de uma faixa anterior não sobrescreve os dados da mais nova.
    if (currentTrackId === track.trackId) return;
    if (lastPausedBroadcastTrackId !== track.trackId) return;

    const artUrl = art?.url || null;
    if (currentAlbumArtUrl !== artUrl) {
      currentAlbumArtUrl = artUrl;
      if (artUrl) {
        server.broadcastLog('ALBUMART', `Loaded cover art for ${track.trackName} (paused)`);
      } else {
        server.broadcastLog('ALBUMART', `No cover art found for ${track.trackName} (paused)`);
      }
    }

    if (lyrics && lyrics.lines && lyrics.lines.length > 0) {
      lyricsSynced = lyrics.synced !== false;
      lyricsVersion++;
      currentLyrics = lyrics.lines;
      currentLyricIndex = -1;
      lyricsState = 'active';
      lyricsSourceStatus = 'connected';
      server.broadcastLog('LYRICS', `Loaded ${lyrics.lines.length} lines for ${track.trackName} (paused)`);
    } else {
      lyricsState = 'empty';
      lyricsSourceStatus = 'disconnected';
      currentLyrics = [];
      currentLyricIndex = -1;
    }

    server.broadcastState();
  } catch (err) {
    console.error('[PausedTrack] Error:', err?.message ?? err);
  }
}

// ── Créditos da faixa (Genius) ─────────────────────────────────────────────────

/**
 * Busca créditos (feats/produtores/escritores) da faixa em background e
 * transmite quando resolvem. Guards anti-race: descarta resultado se a faixa
 * atual já mudou (currentTrackId/pausedDataTrackId) no momento da resolução.
 * @param {object} track Dados da faixa
 */
async function loadTrackCredits(track) {
  if (!track || !track.trackId || !track.trackName || !track.artistName) return;
  const credits = await getTrackCredits(track.trackName, track.artistName).catch(() => null);
  if (!credits) return;

  // Descarta se a faixa que disparou a busca não é mais a atual (tocando OU pausada)
  if (currentTrackId !== track.trackId && pausedDataTrackId !== track.trackId) return;
  creditsVersion++;
  currentCredits = credits;
  server.broadcastLog('CREDITS', `Loaded credits for ${track.trackName} (${track.artistName})`);
  server.broadcastState();
}

// ── Abrir navegador ───────────────────────────────────────────────────────────

function openBrowser(url) {
  if (IN_ELECTRON) return;

  setTimeout(() => {
    if (process.platform === 'win32') {
      spawn('cmd.exe', ['/c', 'start', '', url], {
        windowsHide: true,
        detached: true,
        stdio: 'ignore',
      }).unref();
    } else if (process.platform === 'darwin') {
      spawn('open', [url], { detached: true, stdio: 'ignore' }).unref();
    } else {
      spawn('xdg-open', [url], { detached: true, stdio: 'ignore' }).unref();
    }
  }, 500);
}

// ── Entry point ───────────────────────────────────────────────────────────────

async function main() {
  if (!acquireLock()) {
    console.error('');
    console.error('╔══════════════════════════════════════════════════════════╗');
    console.error('║  ✋ Outra instância já está rodando!                    ║');
    console.error('║                                                        ║');
    console.error('║  O Discord Lyrics Status já está em execução.          ║');
    console.error('║  Verifique a bandeja do sistema (system tray) ou       ║');
    console.error('║  encerre o processo anterior antes de iniciar outro.   ║');
    console.error('╚══════════════════════════════════════════════════════════╝');
    console.error('');

    // Grava o motivo em backend.log — o Electron lê esse arquivo para montar
    // o diálogo de erro. Ajuda a diagnosticar lock stale / PID reciclado.
    // Mantém o arquivo limitado a ~64KB para não crescer sem limite.
    try {
      const { appendFileSync, mkdirSync, statSync } = await import('fs');
      const { join } = await import('path');
      const { homedir } = await import('os');
      const dir = join(process.env.APPDATA || join(homedir(), '.config'), 'discord-lyrics-status');
      mkdirSync(dir, { recursive: true });
      const logPath = join(dir, 'backend.log');
      try {
        if (statSync(logPath).size > 64 * 1024) {
          const { truncateSync } = await import('fs');
          truncateSync(logPath, 0);
        }
      } catch {}
      appendFileSync(
        logPath,
        `[${new Date().toISOString()}] acquireLock FAILED (exit 0) — lock ativo. ` +
        `Se nenhuma janela abriu, o lock é stale (kill forçado): rode 'npm run kill-app' ` +
        `ou apague instance.lock em ${dir}\n`,
        'utf8'
      );
    } catch {}

    process.exit(0);
    return;
  }

  await server.start();
  if (!server.port) {
    console.error('[Server] Could not start HTTP server.');
    process.exit(1);
    return;
  }

  // Registra handler de erro do SMTC para broadcast ao dashboard
  setSmtcErrorHandler((type, message) => {
    console.error(`[SMTC-${type}]`, message);
    server.broadcastLog('SMTC', `${type}: ${message}`);
  });

  if (IN_ELECTRON) {
    const msg = { type: 'server-ready', url: server.url };
    // utilityProcess.fork() usa parentPort em vez de process.send()
    if (typeof process.parentPort?.postMessage === 'function') {
      process.parentPort.postMessage(msg);
    } else if (typeof process.send === 'function') {
      process.send(msg);
    }
  }

  try {
    const { writeFileSync, mkdirSync } = await import('fs');
    const { join } = await import('path');
    const { homedir } = await import('os');
    const urlDir = join(process.env.APPDATA || join(homedir(), '.config'), 'discord-lyrics-status');
    mkdirSync(urlDir, { recursive: true });
    writeFileSync(join(urlDir, 'server.url'), server.url, 'utf8');
  } catch {}

  refreshDiscordAccount();

  // ── Adaptive Sync: ajusta syncLeadMs automaticamente ─────────────────
  // Baseado na latência observada das requisições PATCH para a API do Discord.
  // Usa EWMA filter para suavizar as medições.
  //
  // Limites de segurança:
  //   Mínimo: 50ms  — nunca menos que isso, evita letras atrasadas
  //   Máximo: 500ms — nunca mais que isso, evita letras muito adiantadas
  //
  // Só ajusta após o período de aquecimento (3 primeiras amostras) e
  // apenas quando a mudança é >= 20ms para evitar micro-ajustes.

  onDiscordLatencyChange(function(latencyMs) {
    if (!adaptiveSyncEnabled) return;

    // Clampa dentro dos limites seguros
    const newLead = Math.max(50, Math.min(500, latencyMs));
    const prevLead = currentSyncLeadMs;

    // Evento leve ANTES do early-return: o readout de latência na UI precisa
    // atualizar SEMPRE que a latência muda (>=20ms), mesmo quando o lead
    // derivado varia menos de 10ms. O callback já é throttled pelo status.js.
    server.broadcast('sync-metrics', { leadMs: prevLead, latencyMs, adaptiveSyncEnabled });

    // Só aplica se mudou pelo menos 10ms (evita oscilação)
    if (Math.abs(newLead - prevLead) < 10) return;

    currentSyncLeadMs = newLead;
    if (scheduler) scheduler.setLeadMs(currentSyncLeadMs);

    server.broadcastLog('ADAPTIVE', `Sync lead auto-adjusted: ${prevLead}ms → ${newLead}ms (Discord latency: ${latencyMs}ms)`);
    server.broadcastState();
    // Reenvia com o lead já aplicado para a UI refletir o valor exato
    server.broadcast('sync-metrics', { leadMs: currentSyncLeadMs, latencyMs, adaptiveSyncEnabled });
  });

  // Callback para configurações do frontend (sincronia manual, etc.)
  // Quando o usuário ajusta manualmente, DESATIVA o adaptive sync
  // para não conflitar com a preferência do usuário.
  server.onSettingsChange(function(settings) {
    if (settings.syncLeadMs !== undefined) {
      // Se o valor veio do usuário (não do adaptive sync), desativa
      if (adaptiveSyncEnabled) {
        adaptiveSyncEnabled = false;
        server.broadcastLog('ADAPTIVE', 'Disabled by manual adjustment');
      }
      currentSyncLeadMs = settings.syncLeadMs;
      if (scheduler) scheduler.setLeadMs(currentSyncLeadMs);
      server.broadcastLog('SETTINGS', 'Sync lead changed to ' + currentSyncLeadMs + 'ms');
      // Força broadcast do estado completo para o frontend refletir o novo lead
      server.broadcastState();
      // Atualiza o readout ao vivo na seção Sincronia (agora em modo Manual)
      server.broadcast('sync-metrics', { leadMs: currentSyncLeadMs, latencyMs: getEstimatedLatencyMs(), adaptiveSyncEnabled: false });
    }
    // `adaptiveSync: true` = clique em "Reativar ajuste automático" na UI
    if (settings.adaptiveSync === true) {
      adaptiveSyncEnabled = true;
      // Aplica imediatamente o lead derivado da latência atual (não espera o
      // próximo onDiscordLatencyChange, que pode demorar se a latência estiver
      // estável — o callback só dispara com mudança >= 20ms).
      const latencyNow = getEstimatedLatencyMs();
      const reenabledLead = Math.max(50, Math.min(500, latencyNow));
      currentSyncLeadMs = reenabledLead;
      if (scheduler) scheduler.setLeadMs(currentSyncLeadMs);
      server.broadcastLog('ADAPTIVE', 'Re-enabled by user — lead set to ' + reenabledLead + 'ms (latency ' + latencyNow + 'ms)');
      server.broadcastState();
      server.broadcast('sync-metrics', { leadMs: currentSyncLeadMs, latencyMs: latencyNow, adaptiveSyncEnabled: true });
    }
    if (typeof settings.ecoMode === 'boolean') {
      ecoMode = settings.ecoMode;
      try { saveSetting('ecoMode', ecoMode); } catch {}
      // Ajusta o tick do scheduler em tempo real
      if (scheduler) {
        scheduler.setTickIntervalMs(ecoMode ? SCHEDULER_TICK_ECO_MS : SCHEDULER_TICK_MS);
      }
      server.broadcastLog('SETTINGS', 'Eco mode ' + (ecoMode ? 'ON' : 'OFF'));
      server.broadcastState();
    }
    // `testEmotes: true` = clique em "Testar agora": aplica o emote ao status
    // AGORA, mas NÃO persiste no config (ver o resultado antes de salvar).
    const isEmoteTest = settings.testEmotes === true;
    if (typeof settings.trackEmoji === 'string') {
      const val = settings.trackEmoji.trim();
      if (isEmoteTest) {
        if (currentTrackId && (currentLyricIndex < 0 || firstLyricAfterTrackChange)) {
          try { setCustomStatus(_lastTrackName + ' — ' + _lastArtistName, val || pickRandom(TRACK_EMOJIS)); } catch {}
          server.broadcastLog('SETTINGS', 'TEST: track emoji applied to Discord status');
        } else {
          server.broadcastLog('SETTINGS', 'TEST: track emoji not applied (no active track status)');
        }
      } else {
        customTrackEmoji = val || null;
        try { saveSetting('trackEmoji', customTrackEmoji); } catch {}
        // Re-envia o status atual com o novo emoji (o guard compara texto+emoji)
        if (currentTrackId && (currentLyricIndex < 0 || firstLyricAfterTrackChange)) {
          try {
            setCustomStatus(_lastTrackName + ' — ' + _lastArtistName, getTrackEmoji());
          } catch {}
        }
        server.broadcastLog('SETTINGS', 'Track emoji ' + (customTrackEmoji ? 'updated' : 'reset to random'));
        server.broadcastState();
      }
    }
    if (typeof settings.lyricsEmoji === 'string') {
      const val = settings.lyricsEmoji.trim();
      if (isEmoteTest) {
        if (currentTrackId && currentLyricIndex >= 0 && currentLyrics[currentLyricIndex]) {
          try { setCustomStatus(currentLyrics[currentLyricIndex].text, val || pickRandom(LYRICS_EMOJIS)); } catch {}
          server.broadcastLog('SETTINGS', 'TEST: lyrics emoji applied to Discord status');
        } else {
          server.broadcastLog('SETTINGS', 'TEST: lyrics emoji not applied (no active lyric line)');
        }
      } else {
        customLyricsEmoji = val || null;
        try { saveSetting('lyricsEmoji', customLyricsEmoji); } catch {}
        // Re-envia a linha de letra atual com o novo emoji
        if (currentTrackId && currentLyricIndex >= 0 && currentLyrics[currentLyricIndex]) {
          try {
            setCustomStatus(currentLyrics[currentLyricIndex].text, getLyricsEmoji());
          } catch {}
        }
        server.broadcastLog('SETTINGS', 'Lyrics emoji ' + (customLyricsEmoji ? 'updated' : 'reset to random'));
        server.broadcastState();
      }
    }
    // `revertEmotes: true` = clique em "Restaurar": reaplica os emotes SALVOS
    // ao status atual (desfaz um teste que foi aplicado).
    if (settings.revertEmotes === true) {
      if (currentTrackId && currentLyricIndex >= 0 && currentLyrics[currentLyricIndex]) {
        try { setCustomStatus(currentLyrics[currentLyricIndex].text, getLyricsEmoji()); } catch {}
      } else if (currentTrackId) {
        try { setCustomStatus(_lastTrackName + ' — ' + _lastArtistName, getTrackEmoji()); } catch {}
      }
      server.broadcastLog('SETTINGS', 'Emotes restored to saved values');
      server.broadcastState();
    }
  });

  // Registra handler para seek (clique na barra de progresso)
  server.setSeekHandler(async function(positionMs) {
    await seekTrack(positionMs);
    // Atualiza progress local imediatamente
    lastProgressMs = positionMs;
    lastPollTime = performance.now();
  });

  attachSetupRoutes(server, (account) => {
    discordAccount = account;
    server.broadcastState();
  });
  server.registerNotFoundHandler();

  ui.banner();
  ui.starting();
  server.broadcastLog('APP', 'Discord Lyrics Status v1.0.0');

  if (!getToken()) {
    discordStatus = 'disconnected';
    appReady = false;
    ui.setupNoToken();
    server.broadcastLog('WARN', 'No Discord token configured — setup required via dashboard');
    server.broadcast('status-changed', { status: 'setup-needed' });

    // Abre a dashboard principal para que o token seja configurado diretamente na UI
    openBrowser(server.url);

    await new Promise((resolve) => {
      const checkInterval = setInterval(() => {
        if (getToken()) {
          clearInterval(checkInterval);
          resolve();
        }
      }, 500);
    });

    discordStatus = 'connected';
    appReady = true;
    ui.setupDone();
    server.broadcastLog('SUCCESS', 'Token configured. Starting monitor.');
    server.broadcast('status-changed', { status: 'ready' });
  } else {
    discordStatus = 'connected';
    appReady = true;
    ui.tokenOk();
    server.broadcastLog('SUCCESS', 'Token loaded. Starting monitor.');
    openBrowser(server.url);
  }

  ui.statusBar(!!getToken(), currentPlatform);
  server.broadcastState();

  await poll();
}

function shutdown() {
  ui.shuttingDown();
  server.broadcastLog('APP', 'Shutting down...');
  if (pollTimer) clearTimeout(pollTimer);
  if (debounceTimer) clearTimeout(debounceTimer);
  if (scheduler) {
    try { scheduler.stop(); } catch {}
    scheduler = null;
  }
  clearCustomStatus();
  releaseLock();
  server.stop().catch(() => {});
}

process.on('exit', () => {
  if (pollTimer) clearTimeout(pollTimer);
  releaseLock();
});

process.on('SIGINT', () => { shutdown(); process.exit(0); });
process.on('SIGTERM', () => { shutdown(); process.exit(0); });
process.on('unhandledRejection', (reason) => {
  console.error('[Unhandled Rejection]', reason);
  shutdown();
  process.exit(1);
});
process.on('uncaughtException', (err) => {
  console.error('[Uncaught Exception]', err);
  shutdown();
  process.exit(1);
});

main();
