// ── Transição de tema: gerenciada na seção "Theme Selector" (overlay único) ──

// ── Document Title (setado só quando muda — o título não depende do progresso) ──
/** Último document.title gravado — evita reescrever o título a cada broadcast. */
let _lastDocTitle = null;
function setDocTitle(t) {
  if (t !== _lastDocTitle) {
    _lastDocTitle = t;
    document.title = t;
  }
}

// ── Debug Logger ─────────────────────────────────────────────────────────────
const DBG = {
  /**
   * Logs detalhados: OFF por padrão (economiza CPU/console no dashboard).
   * Ativa com ?debug=1 na URL (ex: http://127.0.0.1:18542/?debug=1).
   * @type {boolean}
   */
  VERBOSE: /[?&]debug=1/.test(location.search),
  /** Prefixo para todos os logs de depuração (fácil de filtrar no console) */
  PREFIX: '🐛',
  /**
   * Log estruturado com timestamp preciso.
   * @param {string} tag - Categoria do log (INIT, WS, STATE, LYRICS, UI)
   * @param {string} msg - Mensagem descritiva
   * @param {*} [data] - Dados opcionais para inspeção
   */
  log(tag, msg, data) {
    if (!this.VERBOSE) return;
    const ts = performance.now().toFixed(1);
    if (data !== undefined) {
      console.log(`[${ts}ms] ${this.PREFIX} [${tag}] ${msg}`, data);
    } else {
      console.log(`[${ts}ms] ${this.PREFIX} [${tag}] ${msg}`);
    }
  },
  /** @param {string} tag */
  warn(tag, msg, data) {
    if (data !== undefined) {
      console.warn(`[DBG] [${tag}] ${msg}`, data);
    } else {
      console.warn(`[DBG] [${tag}] ${msg}`);
    }
  },
  /** @param {string} tag */
  error(tag, msg, data) {
    if (data !== undefined) {
      console.error(`[DBG] [${tag}] ${msg}`, data);
    } else {
      console.error(`[DBG] [${tag}] ${msg}`);
    }
  },
};

// ── Script Loaded ────────────────────────────────────────────────────────────
DBG.log('INIT', `Script version ${document.querySelector('.logo-version')?.textContent || '?'} loaded`);
DBG.log('INIT', `UA: ${navigator.userAgent}`);
DBG.log('INIT', `Platform: ${navigator.platform}, Language: ${navigator.language}`);

// ── State ─────────────────────────────────────────────────────────────────────
const state = {
  platform: 'unknown',
  trackName: null,
  artistName: null,
  albumArtUrl: null,
  progressMs: 0,
  durationMs: 0,
  isPlaying: false,
  lyrics: [],
  lyricsSynced: true,
  currentLyricIndex: -1,
  lyricsCount: 0,
  lyricsVersion: 0,
  discordStatus: 'checking',
  smtcStatus: 'checking',
  lyricsSourceStatus: 'checking',
  lyricsState: 'idle',
  tokenConfigured: false,
  appReady: false,
  discordAccountName: null,
  discordAccountTag: null,
  discordAccountAvatarUrl: null,
  /** Emote personalizado da faixa (null = aleatório) — vem do backend */
  trackEmoji: null,
  /** Emote personalizado das letras (null = aleatório) — vem do backend */
  lyricsEmoji: null,
  /** Antecipação de letras em ms (backend) — atualizada via state e sync-metrics */
  syncLeadMs: 200,
  /** Latência média estimada da API do Discord em ms (backend, EWMA) */
  discordLatencyMs: null,
  /** true = adaptive sync ativo (lead ajustado sozinho); false = Manual (slider) */
  adaptiveSyncEnabled: true,
  /** Flag: true após o primeiro broadcast de state do backend — evita flash falso do banner no load */
  _stateReceived: false,
  // ── Local progress interpolation ────────────────────────────────────
  /** @type {number} Timestamp (performance.now) da última vez que recebemos progressMs */
  lastProgressTime: 0,
  /** @type {number|null} ID da RAF para animação da barra de progresso */
  progressAnimFrame: null,
  /** @type {number|null} Posição exibida suavizada (ms) durante o reajuste */
  smoothMs: null,
  /** @type {boolean} true enquanto a barra desliza para reconciliar com o backend */
  resyncActive: false,
  /** @type {number} Posição inicial do glide (ms) — definida quando resyncActive é ativado */
  resyncFrom: 0,
  /** @type {number} Timestamp (performance.now) do início do glide */
  resyncStartAt: 0,
  /** @type {number|null} Timer do glide de letras (passos intermediários) */
  lyricGlideTimer: null,
  /** Flag: true quando a primeira letra após troca de faixa já disparou */
  _firstLyricAfterTrack: true,
};

// ── DOM References ────────────────────────────────────────────────────────────
const $ = (sel) => document.querySelector(sel);
const els = {
  trackName: $('#track-name'),
  artistName: $('#artist-name'),
  trackCredits: $('#track-credits'),
  albumArt: $('#album-art'),
  albumArtImg: $('#album-art-img'),
  albumArtGhost: $('#album-art-ghost'),
  albumPlaceholder: $('#album-placeholder'),
  waveformBars: $('#waveform-bars'),
  waveformLit: $('#waveform-lit'),
  waveformPreview: $('#waveform-preview'),
  progressBarBg: $('#progress-bar-bg'),
  progressHover: $('#progress-hover'),
  progressTooltip: $('#progress-tooltip'),
  timeCurrent: $('#time-current'),
  timeTotal: $('#time-total'),
  lyricsContainer: $('#lyrics-container'),
  lyricsList: $('#lyrics-list'),
  lyricsEmpty: $('#lyrics-empty'),
  lyricsNoSync: $('#lyrics-no-sync'),
  lyricsStatus: $('#lyrics-status'),
  lyricsBg: $('#lyrics-bg'),
  lyricsSection: document.querySelector('.lyrics-section'),
  emptyText: $('#empty-text'),
  emptySubtext: $('#empty-subtext'),
  emptyTips: $('#empty-tips'),
  platformBadge: $('#platform-badge'),
  platformName: $('#platform-name'),
  logContainer: $('#log-container'),
  logClear: $('#log-clear'),
  logCount: $('#log-count'),
  equalizer: $('#equalizer'),
  vinylRecord: $('#vinyl-record'),
  npIndicator: $('#np-indicator'),
  npLabel: $('#np-label'),
  dotDiscord: $('#dot-discord'),
  dotSmtc: $('#dot-smtc'),
  dotLyrics: $('#dot-lyrics'),
  // Section title
  sectionTitle: $('.section-title'),
  lyricsFooter: $('#lyrics-footer'),
  sourceLrclib: $('#source-lrclib'),
  sourceGenius: $('#source-genius'),
};



// ── Ambient Background (modo ambiente) ──────────────────────────────────────
/**
 * Aplica a capa do álbum como fundo da janela inteira (blur + overlay escuro).
 * Só funciona com body.ambient-on (toggle nas Configurações).
 * @param {string|null} albumArtUrl URL da capa, ou null para limpar
 */
function updateAmbientBg(albumArtUrl) {
  var bg = document.getElementById('ambient-bg');
  if (!bg) return;
  if (albumArtUrl) {
    bg.style.backgroundImage = 'url(' + albumArtUrl + ')';
    bg.classList.add('active');
  } else {
    bg.style.backgroundImage = '';
    bg.classList.remove('active');
  }
}

// ── Lyrics Background Blur ───────────────────────────────────────────────────
function updateLyricsBg(albumArtUrl) {
  var bg = els.lyricsBg;
  var section = els.lyricsSection;
  if (!bg || !section) return;

  if (albumArtUrl) {
    bg.style.backgroundImage = 'url(' + albumArtUrl + ')';
    // Pequeno delay para trigger da transição CSS
    requestAnimationFrame(function() {
      bg.classList.add('loaded');
    });
    section.classList.add('has-bg');
  } else {
    bg.style.backgroundImage = '';
    bg.classList.remove('loaded');
    section.classList.remove('has-bg');
  }
}

// ── Marquee Detection ─────────────────────────────────────────────────────────
/**
 * Verifica se o texto do track name está overflowando e adiciona a classe
 * .marquee para ativar a animação de scroll, ou remove se não precisar.
 */
function updateTrackMarquee() {
  var el = els.trackName;
  if (!el) return;
  // Só ativa marquee se o conteúdo é maior que o container
  if (el.scrollWidth > el.clientWidth) {
    el.classList.add('marquee');
  } else {
    el.classList.remove('marquee');
  }
}

// ── Utility ───────────────────────────────────────────────────────────────────
function formatTime(ms) {
  if (!ms || ms < 0) return '0:00';
  const totalSec = Math.floor(ms / 1000);
  const min = Math.floor(totalSec / 60);
  const sec = totalSec % 60;
  return `${min}:${sec.toString().padStart(2, '0')}`;
}

function escapeHtml(str) {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}

// ── Dynamic Favicon (album art como ícone da aba) ───────────────────────────
/**
 * Atualiza o favicon da página com a capa do álbum atual.
 * Usa um canvas para redimensionar a imagem para 32x32.
 * @param {string|null} imageUrl URL da imagem para usar como favicon, ou null para resetar
 */
function updateFavicon(imageUrl) {
  var link = document.querySelector('link[rel="icon"]') || document.querySelector('link[rel="shortcut icon"]');
  if (!link) {
    link = document.createElement('link');
    link.rel = 'icon';
    document.head.appendChild(link);
  }

  if (!imageUrl) {
    // Reseta para o favicon padrão (SVG inline com o logo do app)
    link.href = 'data:image/svg+xml,' + encodeURIComponent(
      '<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 32 32">'
      + '<rect width="32" height="32" rx="6" fill="#0c0a16"/>'
      + '<circle cx="16" cy="16" r="12" fill="none" stroke="#8b5cf6" stroke-width="1.5" opacity="0.3"/>'
      + '<circle cx="16" cy="16" r="7" fill="none" stroke="#8b5cf6" stroke-width="1.5"/>'
      + '<circle cx="16" cy="16" r="2.5" fill="#8b5cf6"/>'
      + '</svg>'
    );
    return;
  }

  // Cria um canvas para redimensionar a imagem para 32x32
  var img = new Image();
  img.crossOrigin = 'anonymous';
  img.onload = function() {
    try {
      var canvas = document.createElement('canvas');
      canvas.width = 32;
      canvas.height = 32;
      var ctx = canvas.getContext('2d');
      // Desenha com fundo preto arredondado
      ctx.fillStyle = '#0c0a16';
      var r = 4;
      ctx.beginPath();
      ctx.moveTo(r, 0);
      ctx.lineTo(32 - r, 0);
      ctx.quadraticCurveTo(32, 0, 32, r);
      ctx.lineTo(32, 32 - r);
      ctx.quadraticCurveTo(32, 32, 32 - r, 32);
      ctx.lineTo(r, 32);
      ctx.quadraticCurveTo(0, 32, 0, 32 - r);
      ctx.lineTo(0, r);
      ctx.quadraticCurveTo(0, 0, r, 0);
      ctx.closePath();
      ctx.clip();
      // Desenha a imagem centralizada, cobrindo todo o canvas
      var s = Math.min(img.width, img.height);
      var sx = (img.width - s) / 2;
      var sy = (img.height - s) / 2;
      ctx.drawImage(img, sx, sy, s, s, 0, 0, 32, 32);
      link.href = canvas.toDataURL('image/png');
    } catch (e) {
      // Fallback: usa a URL direta (pode não funcionar em todos os casos)
      link.href = imageUrl;
    }
  };
  img.onerror = function() {
    // Fallback silencioso — mantém o favicon atual
  };
  img.src = imageUrl;
}

/**
 * Gera um placeholder SVG com gradiente baseado no nome da faixa.
 * Útil como fallback visual na própria página.
 * @param {string} trackName Nome da faixa para gerar cor consistente
 * @returns {string} Data URL SVG
 */
function generateTrackPlaceholder(trackName) {
  var hash = 0;
  for (var i = 0; i < (trackName || '').length; i++) {
    hash = ((hash << 5) - hash) + trackName.charCodeAt(i);
    hash |= 0;
  }
  var hue = Math.abs(hash) % 360;

  var svg = '<svg xmlns="http://www.w3.org/2000/svg" width="300" height="300" viewBox="0 0 300 300">'
    + '<defs>'
    + '<linearGradient id="pg" x1="0%" y1="0%" x2="100%" y2="100%">'
    + '<stop offset="0%" stop-color="hsl(' + hue + ', 40%, 15%)"/>'
    + '<stop offset="50%" stop-color="hsl(' + ((hue + 40) % 360) + ', 35%, 10%)"/>'
    + '<stop offset="100%" stop-color="hsl(' + ((hue + 80) % 360) + ', 30%, 8%)"/>'
    + '</linearGradient>'
    + '<radialGradient id="pr" cx="50%" cy="50%" r="50%">'
    + '<stop offset="0%" stop-color="hsl(' + hue + ', 50%, 25%)" stop-opacity="0.3"/>'
    + '<stop offset="100%" stop-color="transparent"/>'
    + '</radialGradient>'
    + '</defs>'
    + '<rect width="300" height="300" fill="url(#pg)"/>'
    + '<rect width="300" height="300" fill="url(#pr)"/>'
    + '<circle cx="150" cy="150" r="60" fill="none" stroke="hsl(' + hue + ', 30%, 40%)" stroke-width="1" opacity="0.3"/>'
    + '<circle cx="150" cy="150" r="30" fill="hsl(' + hue + ', 30%, 35%)" opacity="0.2"/>'
    + '<path d="M130 130 L170 150 L130 170 Z" fill="hsl(' + hue + ', 25%, 45%)" opacity="0.15"/>'
    + '</svg>';

  return 'data:image/svg+xml,' + encodeURIComponent(svg);
}

// ── Album Art Crossfade Transition ───────────────────────────────────────────
/** Guarda a URL da arte anterior para fazer crossfade */
var _prevAlbumArtUrl = null;
/** ID do timeout de limpeza do ghost — cancelado em crossfades subsequentes */
var _ghostCleanupTimer = null;

/**
 * Cancel the pending ghost cleanup timer (avoids races on rapid track switches).
 */
function _cancelGhostCleanup() {
  if (_ghostCleanupTimer) {
    clearTimeout(_ghostCleanupTimer);
    _ghostCleanupTimer = null;
  }
}

/**
 * Faz crossfade suave entre a capa antiga e a nova.
 * Estratégia:
 *   1. Ghost recebe a URL antiga e fica visível (z-index acima do main)
 *   2. Img principal recebe a URL nova
 *   3. Ghost fade-out → opacity 0 (450ms)
 *   4. Após a transição, ghost é limpo
 *
 * @param {string} newUrl Nova URL da capa (ou placeholder SVG se sem arte)
 * @param {string|null} [placeholder] Placeholder SVG para usar se a imagem falhar
 */
function crossfadeAlbumArt(newUrl, placeholder) {
  var mainImg = els.albumArtImg;
  var ghost = els.albumArtGhost;
  if (!mainImg || !ghost) return;

  // Modo ambiente: mantém o fundo da janela sincronizado com a capa atual
  updateAmbientBg(state.albumArtUrl);

  // Tema Auto: extrai as cores da capa em QUALQUER caminho (primeira carga,
  // same-URL, crossfade e placeholder) — idempotente e barato fora do auto
  refreshAutoTheme(newUrl);

  // Cancela qualquer limpeza pendente (races de troca rápida)
  _cancelGhostCleanup();

  // Se não tem arte anterior (primeira vez ou estava sem faixa), só aparece
  if (!_prevAlbumArtUrl || _prevAlbumArtUrl === newUrl) {
    ghost.style.display = 'none';
    ghost.src = '';
    ghost.classList.remove('active');

    mainImg.classList.add('img-loading');
    mainImg.src = newUrl;
    mainImg.style.display = 'block';
    _prevAlbumArtUrl = newUrl;

    mainImg.onload = function() {
      mainImg.classList.remove('img-loading');
      mainImg.classList.add('img-loaded');
      mainImg.onload = null;
      mainImg.onerror = null;
      updateFavicon(newUrl);
    };
    mainImg.onerror = function() {
      mainImg.classList.remove('img-loading');
      mainImg.style.display = 'none';
      if (els.albumPlaceholder) {
        els.albumPlaceholder.style.display = 'flex';
        els.albumPlaceholder.style.backgroundImage = 'url(' + placeholder + ')';
        els.albumPlaceholder.style.backgroundSize = 'cover';
      }
      updateFavicon(placeholder || null);
      mainImg.onload = null;
      mainImg.onerror = null;
    };
    if (mainImg.complete && mainImg.naturalWidth > 0) mainImg.onload();
    else if (mainImg.complete && mainImg.naturalWidth === 0) mainImg.onerror();
    return;
  }

  // ── Crossfade: tem arte anterior → transição suave
  // Passo 1: Ghost recebe a arte ANTIGA e fica visível sobre a nova
  ghost.src = _prevAlbumArtUrl;
  ghost.style.display = 'block';
  // Força reflow antes de adicionar a classe para trigger da transição CSS
  void ghost.offsetHeight;
  ghost.classList.add('active');

  // Passo 2: Main img recebe a NOVA arte (abaixo do ghost)
  mainImg.classList.add('img-loading');
  mainImg.src = newUrl;
  mainImg.style.display = 'block';

  mainImg.onload = function() {
    mainImg.classList.remove('img-loading');
    mainImg.classList.add('img-loaded');
    mainImg.onload = null;
    mainImg.onerror = null;
    updateFavicon(newUrl);

    // Passo 3: Ghost fade-out (a nova imagem já está carregada, abaixo)
    ghost.classList.remove('active');

    // Passo 4: Limpa ghost após transição
    _ghostCleanupTimer = setTimeout(function() {
      ghost.src = '';
      ghost.style.display = 'none';
      ghost.classList.remove('active');
      _ghostCleanupTimer = null;
    }, 500);
  };

  mainImg.onerror = function() {
    mainImg.classList.remove('img-loading');
    mainImg.style.display = 'none';
    els.albumArtImg.classList.remove('img-loading', 'img-loaded');
    mainImg.onload = null;
    mainImg.onerror = null;
    if (els.albumPlaceholder) {
      els.albumPlaceholder.style.display = 'flex';
      els.albumPlaceholder.style.backgroundImage = 'url(' + placeholder + ')';
      els.albumPlaceholder.style.backgroundSize = 'cover';
    }
    updateFavicon(placeholder || null);

    // Ghost fade-out mesmo com erro
    ghost.classList.remove('active');
    _ghostCleanupTimer = setTimeout(function() {
      ghost.src = '';
      ghost.style.display = 'none';
      ghost.classList.remove('active');
      _ghostCleanupTimer = null;
    }, 500);
  };

  if (mainImg.complete && mainImg.naturalWidth > 0) {
    mainImg.onload();
  } else if (mainImg.complete && mainImg.naturalWidth === 0) {
    mainImg.onerror();
  }

  _prevAlbumArtUrl = newUrl;
}

// ── Log System ────────────────────────────────────────────────────────────────
/** Tags permitidas no Activity Log — apenas o que é relevante ao Spotify */
const LOG_ALLOWED_TAGS = new Set(['LYRICS', 'ALBUMART', 'SMTC']);

/** Limite máximo de entradas mantidas no DOM (configurável em Config → Desempenho). */
let LOG_LIMIT = 200;

/** Ícones (SVG inline) por tag do Activity Log */
const LOG_TAG_ICONS = {
  'LYRICS': '<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"/></svg>',
  'ALBUMART': '<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="3"/></svg>',
  'SMTC': '<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M19.07 4.93a10 10 0 0 1 0 14.14M15.54 8.46a5 5 0 0 1 0 7.07"/></svg>',
};

function updateLogCount() {
  if (!els.logCount) return;
  const n = els.logContainer.querySelectorAll('.log-entry').length;
  els.logCount.textContent = n > 99 ? '99+' : String(n);
  els.logCount.classList.toggle('has-entries', n > 0);
}

function addLog(level, tag, message) {
  // Filtra apenas logs relacionados ao Spotify/música
  if (!LOG_ALLOWED_TAGS.has(tag.toUpperCase())) return;

  const empty = els.logContainer.querySelector('.log-empty');
  if (empty) empty.remove();

  const time = new Date().toLocaleTimeString('pt-BR', { hour12: false });
  const tagU = tag.toUpperCase();
  const tagCls = tagU.toLowerCase();
  const icon = LOG_TAG_ICONS[tagU] || '';

  const entry = document.createElement('div');
  entry.className = 'log-entry level-' + level;
  entry.innerHTML = [
    '<span class="log-dot" aria-hidden="true"></span>',
    '<span class="log-tag tag-', tagCls, '">', icon, '<span>', escapeHtml(tagU), '</span></span>',
    '<span class="log-msg ', level, '">', escapeHtml(message), '</span>',
    '<span class="log-time">', time, '</span>',
  ].join('');

  els.logContainer.appendChild(entry);

  while (els.logContainer.children.length > LOG_LIMIT) {
    els.logContainer.children[0].remove();
  }

  // Autoscroll inteligente: só segue o fim se o usuário já estiver perto dele
  const nearBottom = els.logContainer.scrollHeight - els.logContainer.scrollTop - els.logContainer.clientHeight < 40;
  if (nearBottom) els.logContainer.scrollTop = els.logContainer.scrollHeight;

  updateLogCount();
}

// ── State Update ──────────────────────────────────────────────────────────────
function updateDot(id, status) {
  const el = document.getElementById(id);
  if (el) el.setAttribute('data-status', status);

  // Update the new status text elements
  const service = id.replace('dot-', '');
  const spText = document.getElementById('sp-text-' + service);
  const stStatus = document.getElementById('st-' + service);

  const statusLabels = {
    'connected': 'Conectado',
    'disconnected': 'Desconectado',
    'checking': 'Verificando...'
  };

  const label = statusLabels[status] || status;

  if (spText) spText.textContent = label;
  if (stStatus) stStatus.textContent = label;

  // Sincroniza o data-status da linha correspondente no painel de conexões
  // para que o CSS controle cores, glow e animações.
  const stRow = stStatus ? stStatus.closest('.st-row') : null;
  if (stRow) stRow.setAttribute('data-status', status);

  updateConnectionsSummary();
}

/**
 * Atualiza o badge resumo no topo do painel de conexões
 * (ex: "Online", "2/3", "Verificando...", "Offline").
 */
function updateConnectionsSummary() {
  const overall = document.getElementById('st-overall');
  const overallText = document.getElementById('st-overall-text');
  if (!overall || !overallText) return;

  const rows = document.querySelectorAll('.status-tooltip .st-row');
  const total = rows.length;
  const connected = Array.from(rows).filter((r) => r.getAttribute('data-status') === 'connected').length;
  const anyChecking = Array.from(rows).some((r) => r.getAttribute('data-status') === 'checking');

  let state = 'disconnected';
  let label = 'Offline';

  if (total > 0 && connected === total) {
    state = 'connected';
    label = 'Online';
  } else if (connected > 0) {
    state = 'checking';
    label = connected + '/' + total;
  } else if (anyChecking) {
    state = 'checking';
    label = 'Verificando...';
  }

  overall.setAttribute('data-status', state);
  overallText.textContent = label;
}

/**
 * Atualiza o rodapé do WebSocket no painel de conexões.
 */
function updateWsFooter() {
  const footer = document.querySelector('#status-tooltip .st-footer');
  const wsDot = document.getElementById('st-ws-dot');
  const wsStatus = document.getElementById('st-ws-status');
  if (!wsDot || !wsStatus) return;

  let state;
  let label;
  if (ws && ws.readyState === WebSocket.OPEN) {
    state = 'connected';
    label = 'Conectado';
  } else if (ws && ws.readyState === WebSocket.CONNECTING) {
    state = 'checking';
    label = 'Conectando...';
  } else {
    state = 'disconnected';
    label = 'Desconectado';
  }

  if (footer) footer.setAttribute('data-status', state);
  wsDot.setAttribute('data-status', state);
  wsStatus.textContent = label;
}

/**
 * Renderiza os créditos da faixa (artistas completos/feats, produtores,
 * escritores) abaixo do nome do artista, acima da barra de progresso.
 * Preenchido assíncronamente quando o backend resolve os créditos (Genius).
 * @param {object|null} data estado do broadcast (usa data.credits / data.artistName)
 */
function renderTrackCredits(data) {
  const el = els.trackCredits;
  if (!el) return;
  const credits = data && data.credits;
  if (!credits || (!credits.artistNames && !credits.featured && !credits.producers && !credits.writers)) {
    el.innerHTML = '';
    el.classList.remove('visible');
    return;
  }
  const parts = [];
  // Lista completa de artistas (inclui feats que o SMTC não reporta)
  const artistLower = String(data && data.artistName || '').toLowerCase().trim();
  if (credits.artistNames && credits.artistNames.toLowerCase().trim() !== artistLower) {
    parts.push('<span class="credit-chip credit-artists">' + escapeHtml(credits.artistNames) + '</span>');
  }
  if (credits.featured && credits.featured.length) {
    parts.push('<span class="credit-chip credit-feat">feat. ' + escapeHtml(credits.featured.join(', ')) + '</span>');
  }
  if (credits.producers && credits.producers.length) {
    parts.push('<span class="credit-chip credit-prod">🎛 ' + escapeHtml(credits.producers.join(', ')) + '</span>');
  }
  if (credits.writers && credits.writers.length) {
    parts.push('<span class="credit-chip credit-writer">✍️ ' + escapeHtml(credits.writers.join(', ')) + '</span>');
  }
  el.innerHTML = parts.join('');
  el.classList.toggle('visible', parts.length > 0);
}

function updateUI(data) {
  DBG.log('UI', 'updateUI called' + (data?.trackName ? ` — ${data.trackName} — ${data.artistName}` : ' (no track)'));
  if (!data) {
    DBG.warn('UI', 'updateUI received null/undefined data');
    return;
  }

  // Primeiro broadcast de state recebido — só a partir daqui o banner de
  // diagnóstico pode avaliar os modos token/smtc/paused sem risco de flash
  // falso durante o carregamento (token já configurado, etc.).
  state._stateReceived = true;

  // Guarda referências para comparar mudança de faixa
  const prevTrackId = state.trackName && state.artistName
    ? (state.trackName + '::' + state.artistName).toLowerCase()
    : null;
  const newTrackId = data.trackName && data.artistName
    ? (data.trackName + '::' + data.artistName).toLowerCase()
    : null;

  state.discordStatus = data.discordStatus || 'checking';
  state.smtcStatus = data.smtcStatus || 'checking';
  state.lyricsSourceStatus = data.lyricsSourceStatus || 'checking';
  state.lyricsState = data.lyricsState || 'idle';
  state.tokenConfigured = data.tokenConfigured !== false;
  state.appReady = data.appReady !== false;
  state.trackEmoji = data.trackEmoji || null;
  state.lyricsEmoji = data.lyricsEmoji || null;
  state.discordAccountName = data.discordAccountName || null;
  state.discordAccountTag = data.discordAccountTag || null;
  state.discordAccountAvatarUrl = data.discordAccountAvatarUrl || null;

  // Readout ao vivo da seção Sincronia (sync lead + latência + modo do adaptive sync)
  if (typeof data.syncLeadMs === 'number') state.syncLeadMs = data.syncLeadMs;
  if (typeof data.discordLatencyMs === 'number') state.discordLatencyMs = data.discordLatencyMs;
  if (typeof data.adaptiveSyncEnabled === 'boolean') state.adaptiveSyncEnabled = data.adaptiveSyncEnabled;
  updateSyncMetrics();

  updateDot('dot-discord', state.discordStatus);
  updateDot('dot-smtc', state.smtcStatus);

  // ── Pause badge visibility ──────────────────────────────────────────
  var pauseBadge = document.getElementById('pause-badge');
  var pauseIndicator = document.getElementById('pause-indicator');
  function _showPauseBadge(show) {
    if (!pauseBadge) return;
    pauseBadge.classList.toggle('visible', show);
    if (pauseIndicator) {
      pauseIndicator.classList.toggle('visible', show);
    }
  }

  document.body.classList.remove('no-track');

  if (data.trackName && data.artistName && data.isPlaying) {
    _showPauseBadge(false);
    if (els.albumArt) {
      els.albumArt.classList.remove('paused');
      els.albumArt.classList.add('is-playing');
    }
    // Vinyl record animation
    if (els.vinylRecord) {
      els.vinylRecord.classList.add('visible', 'spinning');
      els.vinylRecord.classList.remove('paused');
    }
    state.trackName = data.trackName;
    state.artistName = data.artistName;
    state.platform = data.platform || 'unknown';
    // Divergência: compara a posição local interpolada com a posição autoritativa
    // do backend ANTES de sobrescrever.
    const localNow = (state.progressMs || 0) + (performance.now() - state.lastProgressTime);
    const sameTrack = (newTrackId === prevTrackId);
    const backendProgress = data.progressMs || 0;
    state.durationMs = data.durationMs || 0;
    state.isPlaying = true;

    // ── Correção de drift (anti-pulo) ────────────────────────────────────
    // O backend envia progressMs a cada ~1s e o daemon SMTC frequentemente o
    // reporta QUANTIZADO em segundos inteiros. Sobrescrever o âncora local com
    // esse valor direto fazia a barra "pular" (recuar e re-avançar) a cada
    // mensagem — o clássico dente de serra. Estratégia:
    //   • Drift GRANDE À FRENTE (> RESYNC_THRESHOLD_MS — stall, seek externo,
    //     reconexão): reajuste suave existente (glide easeOutCubic).
    //   • Drift GRANDE PARA TRÁS (> RESYNC_BACKWARD_TOLERANCE_MS — seek/restart
    //     de verdade): também reconcilia (volta ao ponto correto).
    //   • Drift PEQUENO OU BACKEND LEVEMENTE ATRÁS (quantização do SMTC em
    //     segundos inteiros + atraso de 1–3s na troca de faixa): re-ancora na
    //     posição local contínua. O relógio local (progressMs + elapsed) é de
    //     parede real — manter o âncora é tão preciso quanto o backend. Antes,
    //     qualquer |drift| > 1200ms (incluindo backend atrás) puxava a barra/anel
    //     de volta, travando o anel em ~0% no início da faixa.
    if (sameTrack && state.durationMs > 0) {
      const drift = backendProgress - localNow;
      // Assimetria anti-travamento: backend atrás dentro da tolerância NUNCA
      // reconcilia (é lag de quantização/transição, não seek). Só volta em
      // saltos grandes reais. Trade-off aceito: seeks externos pequenos para
      // trás (< 3s) não refletem no anel/barra — o scheduler de letras do
      // backend recalibra sozinho, e o drag na barra ancora direto (doSeek).
      const backendLagging = drift < 0 && drift >= -RESYNC_BACKWARD_TOLERANCE_MS;
      if (!backendLagging && Math.abs(drift) > RESYNC_THRESHOLD_MS) {
        state.resyncActive = true;
        state.resyncFrom = state.smoothMs !== null ? state.smoothMs : localNow;
        state.resyncStartAt = performance.now();
        state.progressMs = backendProgress;
        state.lastProgressTime = performance.now();
        updateProgressSyncStatus();
      } else {
        // Continuidade pura: atualiza o âncora para a posição já exibida.
        state.progressMs = localNow;
        state.lastProgressTime = performance.now();
      }
    } else {
      // Primeira faixa / troca de faixa: âncora vem do backend mesmo.
      state.progressMs = backendProgress;
      state.lastProgressTime = performance.now();
    }

    // ── Now Playing Indicator ──────────────────────────────────────
    if (els.npIndicator) {
      els.npIndicator.classList.add('np-visible');
      els.npIndicator.classList.remove('np-paused');
    }
    if (els.npLabel) els.npLabel.textContent = data.trackName;

    // ── Detects track change ────────────────────────────────────────────
    var isTrackChange = (newTrackId !== prevTrackId);
    if (isTrackChange) {
      // Waveform com padrão único para cada faixa
      buildWaveform(data.trackName + '::' + data.artistName);
      // Reseta flag para que o efeito visual dispare na próxima leva de letras
      state._firstLyricAfterTrack = true;
      // Nova faixa: zera o estado de reajuste (a posição recomeça do zero)
      state.smoothMs = data.progressMs || 0;
      state.resyncActive = false;
      cancelLyricGlide();
      updateProgressSyncStatus();
      // Limpa glow pendente de faixa anterior (se houver)
      if (state._glowTimeout) {
        clearTimeout(state._glowTimeout);
        state._glowTimeout = null;
      }
      // ── Smooth fade-out / fade-in transition ───────────────────────
      var playerSection = document.querySelector('.player-section');
      var lyricsSection = document.querySelector('.lyrics-section');

      // Fade out ambos os painéis
      if (playerSection) playerSection.classList.add('track-fade-out');
      if (lyricsSection) lyricsSection.classList.add('track-fade-out');

      // Após o fade-out (250ms), atualiza conteúdo e faz fade-in
      setTimeout(function() {
        // ── Atualiza player section ─────────────────────────────────
        els.trackName.textContent = data.trackName;
        els.artistName.textContent = data.artistName;
        els.albumArt.classList.add('has-track');
        document.querySelector('.player-section')?.classList.add('has-track');
        updateTrackMarquee();
        setDocTitle(data.trackName + ' - ' + data.artistName + ' - Discord Lyrics Status');

        var placeholder = generateTrackPlaceholder(data.trackName);
        if (data.albumArtUrl) {
          state.albumArtUrl = data.albumArtUrl;
          crossfadeAlbumArt(data.albumArtUrl, placeholder);
          els.albumPlaceholder.style.display = 'none';
          els.albumPlaceholder.style.backgroundImage = '';
        } else {
          state.albumArtUrl = null;
          // Crossfade para placeholder se havia arte anterior
          if (_prevAlbumArtUrl) {
            crossfadeAlbumArt(placeholder, placeholder);
          } else {
            els.albumArtImg.style.display = 'none';
            els.albumArtImg.classList.remove('img-loading', 'img-loaded');
          }
          els.albumPlaceholder.style.display = 'flex';
          els.albumPlaceholder.style.backgroundImage = '';
          els.albumPlaceholder.style.backgroundImage = 'url(' + placeholder + ')';
          els.albumPlaceholder.style.backgroundSize = 'cover';
          updateFavicon(placeholder);
        }

        // Atualiza background blur do painel de letras
        updateLyricsBg(state.albumArtUrl);

        if (data.durationMs > 0) {
          state.lastProgressTime = performance.now();
          els.timeTotal.textContent = formatTime(data.durationMs);
          if (els.progressBarBg) { els.progressBarBg.classList.remove('paused'); els.progressBarBg.classList.add('playing'); }
          startProgressAnimation();
        }

        if (data.platform && data.platform !== 'unknown') {
          els.platformName.textContent = data.platform.toUpperCase();
          els.platformBadge.classList.add('has-platform');
        } else {
          els.platformBadge.classList.remove('has-platform');
          els.platformName.textContent = 'SPOTIFY';
        }

        // Remove fade-out, aplica fade-in
        if (playerSection) {
          playerSection.classList.remove('track-fade-out');
          playerSection.classList.add('track-fade-in');
        }
        if (lyricsSection) {
          lyricsSection.classList.remove('track-fade-out');
          lyricsSection.classList.add('track-fade-in');
        }

        // Remove fade-in classes após animação
        setTimeout(function() {
          if (playerSection) playerSection.classList.remove('track-fade-in');
          if (lyricsSection) lyricsSection.classList.remove('track-fade-in');
        }, 500);
      }, 280);
    } else {
      // ── Sem mudança de faixa — atualiza normalmente ────────────────
      els.trackName.textContent = data.trackName;
      els.artistName.textContent = data.artistName;
      els.albumArt.classList.add('has-track');
      document.querySelector('.player-section')?.classList.add('has-track');
      setDocTitle(data.trackName + ' - ' + data.artistName + ' - Discord Lyrics Status');

      const artChanged = data.albumArtUrl !== undefined && data.albumArtUrl !== state.albumArtUrl;
      if (artChanged) {
        state.albumArtUrl = data.albumArtUrl || null;
        var p = generateTrackPlaceholder(data.trackName || state.trackName);
        if (data.albumArtUrl) {
          crossfadeAlbumArt(data.albumArtUrl, p);
          els.albumPlaceholder.style.display = 'none';
          els.albumPlaceholder.style.backgroundImage = '';
        } else {
          if (_prevAlbumArtUrl) {
            crossfadeAlbumArt(p, p);
          } else {
            els.albumArtImg.style.display = 'none';
            els.albumArtImg.classList.remove('img-loading', 'img-loaded');
          }
          els.albumPlaceholder.style.display = 'flex';
          els.albumPlaceholder.style.backgroundImage = '';
          els.albumPlaceholder.style.backgroundImage = 'url(' + p + ')';
          els.albumPlaceholder.style.backgroundSize = 'cover';
          updateFavicon(p);
        }
        // O blur do painel de letras SEMPRE segue a capa atual — antes, a arte
        // que chegava assíncrona atualizava o player/ambiente mas deixava o
        // blur das letras com a capa da faixa ANTERIOR.
        updateLyricsBg(state.albumArtUrl);
      }

      if (data.durationMs > 0) {
        state.lastProgressTime = performance.now();
        els.timeTotal.textContent = formatTime(data.durationMs);
        if (els.progressBarBg) { els.progressBarBg.classList.remove('paused'); els.progressBarBg.classList.add('playing'); }
        startProgressAnimation();
      }

      if (data.platform && data.platform !== 'unknown') {
        els.platformName.textContent = data.platform.toUpperCase();
        els.platformBadge.classList.add('has-platform');
      } else {
        els.platformBadge.classList.remove('has-platform');
        els.platformName.textContent = 'SPOTIFY';
      }

      // Créditos (feats/produtores/escritores) abaixo do artista
      renderTrackCredits(data);
    }
  } else if (data.trackName && data.artistName) {
    _showPauseBadge(true);
    // Waveform congelado: sinal visual de pausa na barra de progresso
    if (els.progressBarBg) {
      els.progressBarBg.classList.remove('playing');
      els.progressBarBg.classList.add('paused');
    }
    if (els.albumArt) {
      els.albumArt.classList.add('paused');
      els.albumArt.classList.remove('is-playing');
    }
    // Vinyl record pause
    if (els.vinylRecord) {
      els.vinylRecord.classList.add('paused');
    }
    // ── PAUSED: keep track info, freeze UI ────────────────────────
    const _wasPlaying = state.isPlaying;
    state.isPlaying = false;
    // Faixa pausada pode ter mudado (troca de música durante a pausa):
    // atualiza nome/artista/capa exibidos para refletir a faixa atual.
    const _pausedTrackChanged = (data.trackName !== state.trackName || data.artistName !== state.artistName);
    if (_pausedTrackChanged) {
      // Waveform com o padrão da NOVA faixa (troca de música durante a pausa)
      buildWaveform(data.trackName + '::' + data.artistName);
      state.trackName = data.trackName;
      state.artistName = data.artistName;
      els.trackName.textContent = data.trackName;
      els.artistName.textContent = data.artistName;
      updateTrackMarquee();
      const p = generateTrackPlaceholder(data.trackName);
      if (data.albumArtUrl) {
        state.albumArtUrl = data.albumArtUrl;
        crossfadeAlbumArt(data.albumArtUrl, p);
        els.albumPlaceholder.style.display = 'none';
        els.albumPlaceholder.style.backgroundImage = '';
      } else {
        state.albumArtUrl = null;
        refreshAutoTheme(null);
        els.albumArtImg.style.display = 'none';
        els.albumArtImg.classList.remove('img-loading', 'img-loaded');
        els.albumPlaceholder.style.display = 'flex';
        els.albumPlaceholder.style.backgroundImage = 'url(' + p + ')';
        els.albumPlaceholder.style.backgroundSize = 'cover';
      }
      updateLyricsBg(state.albumArtUrl);
    }
    // ── Arte chegou/atualizou para a MESMA faixa pausada ───────────
    // (ex: busca em background de capa/letras da faixa pausada). Sem isto,
    // a capa nova era ignorada enquanto pausado e o blur ficava na arte antiga.
    if (data.albumArtUrl !== undefined && data.albumArtUrl !== state.albumArtUrl) {
      state.albumArtUrl = data.albumArtUrl || null;
      const p = generateTrackPlaceholder(data.trackName || state.trackName);
      if (data.albumArtUrl) {
        crossfadeAlbumArt(data.albumArtUrl, p);
        if (els.albumPlaceholder) {
          els.albumPlaceholder.style.display = 'none';
          els.albumPlaceholder.style.backgroundImage = '';
        }
      } else {
        if (_prevAlbumArtUrl) {
          crossfadeAlbumArt(p, p);
        } else {
          els.albumArtImg.style.display = 'none';
          els.albumArtImg.classList.remove('img-loading', 'img-loaded');
        }
        if (els.albumPlaceholder) {
          els.albumPlaceholder.style.display = 'flex';
          els.albumPlaceholder.style.backgroundImage = '';
          els.albumPlaceholder.style.backgroundImage = 'url(' + p + ')';
          els.albumPlaceholder.style.backgroundSize = 'cover';
        }
      }
      updateLyricsBg(state.albumArtUrl);
    }
    // ── Now Playing Indicator (paused) ─────────────────────────
    if (els.npIndicator) {
      els.npIndicator.classList.add('np-visible', 'np-paused');
    }
    if (els.npLabel) els.npLabel.textContent = state.trackName || data.trackName;
    state.progressMs = data.progressMs || state.progressMs || 0;
    state.durationMs = data.durationMs || state.durationMs || 0;
    // Troca de faixa durante a pausa: atualiza também a duração total exibida
    if (_pausedTrackChanged && state.durationMs > 0) {
      els.timeTotal.textContent = formatTime(state.durationMs);
    }
    if (_wasPlaying || _pausedTrackChanged) {
      const frozenPct = state.durationMs > 0 ? Math.min(100, (state.progressMs / state.durationMs) * 100) : 0;
      // Pause: congela na posição atual, sem reajuste pendente
      state.smoothMs = state.progressMs;
      state.resyncActive = false;
      cancelLyricGlide();
      updateProgressSyncStatus();
      setProgressPct(frozenPct);
      _lastTimeText = formatTime(state.progressMs);
      els.timeCurrent.textContent = _lastTimeText;
      setArtRingProgress(frozenPct);
      stopProgressAnimation();
    }
    if (state.trackName) {
      setDocTitle(state.trackName + ' - ' + state.artistName + ' (paused) - Discord Lyrics Status');
    }
    // Créditos também quando pausado (a faixa pausada tem seus próprios)
    renderTrackCredits(data);
  } else {
    _showPauseBadge(false);
    if (els.albumArt) {
      els.albumArt.classList.remove('paused', 'is-playing');
    }
    // Hide vinyl record
    if (els.vinylRecord) {
      els.vinylRecord.classList.remove('visible', 'spinning', 'paused');
    }
    state.trackName = null;
    state.artistName = null;
    state.isPlaying = false;

    // ── Now Playing Indicator (hidden) ─────────────────────────
    if (els.npIndicator) els.npIndicator.classList.remove('np-visible', 'np-paused');
    if (els.npLabel) els.npLabel.textContent = 'No track';

    state.albumArtUrl = null;
    refreshAutoTheme(null);
    // Modo ambiente: limpa o fundo quando não há mais faixa
    updateAmbientBg(null);
    els.trackName.textContent = 'No track playing';
    els.artistName.textContent = 'Waiting for music...';
    els.albumArt.classList.remove('has-track');
    document.querySelector('.player-section')?.classList.remove('has-track');
    els.albumArtImg.style.display = 'none';
    els.albumArtImg.src = '';
    els.albumPlaceholder.style.display = 'flex';
    els.albumPlaceholder.style.backgroundImage = '';
    updateFavicon(null);
    setDocTitle('Discord Lyrics Status');
    els.platformBadge.classList.remove('has-platform');
    els.platformName.textContent = 'WAITING...';
    // Sem faixa: esconde os créditos
    renderTrackCredits(null);
    document.body.classList.add('no-track');
    setProgressPct(0);
    state.smoothMs = 0;
    state.resyncActive = false;
    cancelLyricGlide();
    updateProgressSyncStatus();
    _lastTimeText = '0:00';
    els.timeCurrent.textContent = '0:00';
    els.timeTotal.textContent = '0:00';
    setArtRingProgress(0);
    if (els.progressBarBg) els.progressBarBg.classList.remove('playing', 'paused');
    stopProgressAnimation();
  }

  updateDot('dot-lyrics', state.lyricsState === 'active' ? 'connected' : (state.lyricsState === 'empty' ? 'disconnected' : 'checking'));

  // Lyrics
  state.lyricsSynced = data.lyricsSynced !== false;

  if (state.lyricsState === 'active' && data.lyrics && data.lyrics.length > 0) {
    let activeIdx = data.currentLyricIndex >= 0 ? data.currentLyricIndex : state.currentLyricIndex;

    // Para letras NÃO sincronizadas (Genius) OU faixa pausada (sem scheduler
    // ativo dirigindo o índice), calcula a linha atual pela posição de progresso
    // — assim a letra destaca onde a música parou mesmo em pausa.
    if (data.durationMs > 0 && data.progressMs >= 0 && (!state.lyricsSynced || !data.isPlaying)) {
      const pct = Math.min(1, Math.max(0, data.progressMs / data.durationMs));
      activeIdx = Math.min(data.lyrics.length - 1, Math.floor(pct * data.lyrics.length));
    }

    // Detecta se as letras MUDARAM (nova faixa) — anima transição
    const lyricsChanged = data.lyricsVersion !== state.lyricsVersion && data.lyricsVersion > 0;
    if (lyricsChanged) {
      // Novo conjunto de letras: cancela qualquer glide pendente — o glide
      // capturou o array ANTIGO e poderia reconstruir o DOM com conteúdo stale.
      cancelLyricGlide();
      // Só aplica o efeito visual na PRIMEIRA leva de letras após troca de faixa
      if (state._firstLyricAfterTrack) {
        // Glow pulsante na seção de letras
        if (els.lyricsSection) {
          els.lyricsSection.classList.add('lyrics-first-glow');
          if (state._glowTimeout) clearTimeout(state._glowTimeout);
          state._glowTimeout = setTimeout(function() {
            if (els.lyricsSection) els.lyricsSection.classList.remove('lyrics-first-glow');
            state._glowTimeout = null;
          }, 1000);
        }

        state._firstLyricAfterTrack = false;
      }

      // Fade out das letras ANTIGAS
      if (els.lyricsList.children.length > 0 && els.lyricsList.children.length !== data.lyrics.length) {
        els.lyricsList.classList.add('lyrics-fade-out');
        setTimeout(function() {
          renderLyrics(data.lyrics, activeIdx, true);
          els.lyricsList.classList.remove('lyrics-fade-out');
          els.lyricsList.classList.add('lyrics-fade-in');
          setTimeout(function() {
            els.lyricsList.classList.remove('lyrics-fade-in');
          }, 400);
        }, 200);
      } else {
        renderLyrics(data.lyrics, activeIdx, true);
      }
    } else {
      // Se um glide de letras está em andamento, deixa o lyric-line (autoridade
      // de transições) conduzir — evita oscilação com state defasado pós-seek.
      if (!state.lyricGlideTimer) {
        const fromIdx = state.currentLyricIndex;
        // Salto grande apenas nas letras SINCRONIZADAS (o backend é a autoridade).
        // Nas não sincronizadas o tick já segue displayedMs (glide da barra).
        if (fromIdx >= 0 && state.lyricsSynced && Math.abs(activeIdx - fromIdx) > 1) {
          glideLyricsTo(activeIdx);
        } else {
          renderLyrics(data.lyrics, activeIdx, false);
        }
      }
    }

    state.lyrics = data.lyrics;
    state.lyricsVersion = data.lyricsVersion || state.lyricsVersion;
    state.lyricsCount = data.lyrics.length;
    // Se há um glide em andamento, ele controla currentLyricIndex passo a passo
    if (!state.lyricGlideTimer) state.currentLyricIndex = activeIdx;

    // Status text: mostra se está sincronizado ou Genius
    if (state.lyricsSynced) {
      els.lyricsStatus.textContent = '\u266A ' + data.lyrics.length + ' lines synced';
    } else {
      const progressPct = data.durationMs > 0 ? Math.round((data.progressMs / data.durationMs) * 100) : 0;
      els.lyricsStatus.textContent = 'Genius \u00B7 ' + progressPct + '% \u00B7 ' + data.lyrics.length + ' lines';
    }
    // Atualiza título da seção conforme a fonte (inclui badge se existir)
    if (els.sectionTitle) {
      var badgeExists = !!document.querySelector('.lyrics-badge-entrance');
      var badgeHtml = badgeExists ? '<span class="lyrics-badge-entrance visible"><span class="lbe-dot"></span> Letras</span>'
        : '<span class="lyrics-badge-entrance"><span class="lbe-dot"></span> Letras</span>';
      els.sectionTitle.innerHTML = '<span class="title-icon">' + (state.lyricsSynced ? '\u266A' : '\u2605') + '</span> '
        + (state.lyricsSynced ? 'Synced Lyrics' : 'Genius Lyrics')
        + badgeHtml;
      // Se badge acabou de ser inserido, anima entrada com double RAF
      if (!badgeExists) {
        requestAnimationFrame(function() {
          requestAnimationFrame(function() {
            var b = document.querySelector('.lyrics-badge-entrance');
            if (b) b.classList.add('visible');
          });
        });
      }
    }

    // Mostra footer com badges das fontes
    if (els.lyricsFooter) {
      els.lyricsFooter.classList.add('visible');
      if (els.sourceLrclib) els.sourceLrclib.style.display = state.lyricsSynced ? 'flex' : 'none';
      if (els.sourceGenius) els.sourceGenius.style.display = state.lyricsSynced ? 'none' : 'flex';
    }

    els.lyricsStatus.className = 'lyrics-status active' + (state.lyricsSynced ? '' : ' unsynced');
    els.lyricsEmpty.style.display = 'none';
    els.lyricsNoSync.className = 'lyrics-no-sync';
    els.lyricsList.style.display = 'flex';
    els.lyricsList.classList.remove('lyrics-loading');
    els.lyricsList.classList.toggle('lyrics-unsynced', !state.lyricsSynced);
  } else {
    // Esconde footer quando não há letras ativas
    if (els.lyricsFooter) els.lyricsFooter.classList.remove('visible');
    if (els.sectionTitle) {
      els.sectionTitle.innerHTML = '<span class="title-icon">\u266A</span> Synced Lyrics';
    }
  }

  if (state.lyricsState === 'loading') {
    els.lyricsStatus.textContent = 'Loading lyrics...';
    els.lyricsStatus.className = 'lyrics-status loading';
    if (els.lyricsList.children.length > 0) {
      els.lyricsList.classList.add('lyrics-loading');
      els.lyricsList.style.display = 'flex';
      els.lyricsEmpty.style.display = 'none';
    } else {
      els.lyricsList.style.display = 'none';
      els.lyricsEmpty.style.display = 'flex';
    }
    els.lyricsNoSync.className = 'lyrics-no-sync';
  } else if (state.lyricsState === 'empty' && state.trackName) {
    els.lyricsNoSync.className = 'lyrics-no-sync visible';
    els.lyricsStatus.textContent = 'No synced lyrics';
    els.lyricsStatus.className = 'lyrics-status';
    els.lyricsList.style.display = 'none';
    els.lyricsEmpty.style.display = 'none';
  } else if (state.lyricsState !== 'active') {
    els.lyricsStatus.textContent = state.trackName ? 'No synced lyrics' : 'Waiting for track...';
    els.lyricsStatus.className = 'lyrics-status';
    els.lyricsList.style.display = 'none';
    els.lyricsNoSync.className = 'lyrics-no-sync';
    els.lyricsEmpty.style.display = 'flex';
  }

  if (!state.tokenConfigured) {
    showSetupPrompt();
  } else {
    hideSetupPrompt();
  }

  updateDiscordAccountUI();

  // Banner de auto-diagnóstico (backend offline / token / spotify)
  updateDiagBanner();
}

// ── Lyrics Rendering ─────────────────────────────────────────────────────────
function renderLyrics(lyrics, activeIndex, isNewLyrics, skipScroll) {
  if (!lyrics || lyrics.length === 0) {
    DBG.log('LYRICS', 'renderLyrics: empty lyrics, hiding list');
    els.lyricsEmpty.style.display = 'flex';
    els.lyricsList.style.display = 'none';
    els.lyricsList.innerHTML = '';
    return;
  }
  DBG.log('LYRICS', `renderLyrics: ${lyrics.length} lines, active=${activeIndex}, isNew=${!!isNewLyrics}, currentLine=${lyrics[activeIndex]?.text?.slice(0, 40) || '(none)'}`);

  els.lyricsEmpty.style.display = 'none';
  els.lyricsList.style.display = 'flex';

  // Detecta se as letras são novas (mudou a quantidade de linhas)
  var isNewSet = isNewLyrics || (els.lyricsList.children.length !== lyrics.length);

  if (isNewSet) {
    // Remove todas as animações de entrada anteriores
    els.lyricsList.querySelectorAll('.lyric-entering').forEach(function(el) {
      el.classList.remove('lyric-entering');
    });

    // Reconstrói o HTML
    els.lyricsList.innerHTML = lyrics
      .map(function(line, i) {
        return '<div class="lyric-line future" data-index="' + i + '">' + escapeHtml(line.text) + '</div>';
      })
      .join('');

    // Aplica entrada escalonada com animação CSS — cada linha ganha um delay via style
    var newLines = els.lyricsList.children;
    for (var i = 0; i < newLines.length; i++) {
      var _line = newLines[i];
      var _state = 'future';
      if (i < activeIndex) _state = 'past';
      else if (i === activeIndex) _state = 'active';

      _line.className = 'lyric-line ' + _state;
      _line.style.animationDelay = (i * 30) + 'ms';
      _line.classList.add('lyric-entering');

      // Remove a classe de animação depois que termina
      (function(el) {
        setTimeout(function() {
          el.classList.remove('lyric-entering');
          el.style.animationDelay = '';
        }, 600 + i * 30);
      })(_line);
    }

    // Scroll para a linha ativa após as animações
    if (activeIndex >= 0) {
      setTimeout(function() {
        var target = els.lyricsList.querySelector('[data-index="' + activeIndex + '"]');
        if (target) target.scrollIntoView({ block: 'center', behavior: 'smooth' });
      }, 200);
    }
    return;
  }

  // Atualização normal (não é novo conjunto de letras) — transição suave entre estados
  var lines = els.lyricsList.children;
  for (var i = 0; i < lines.length; i++) {
    var line = lines[i];
    var newClass = 'lyric-line';

    if (i < activeIndex) {
      newClass += ' past';
    } else if (i === activeIndex) {
      newClass += ' active';
      // Só faz scroll se for uma linha diferente da atual — e nunca durante
      // um glide de reajuste (skipScroll), para evitar jitter de scroll.
      // Usa IIFE para capturar o valor correto de `line` no closure
      if (!skipScroll && (i !== state.currentLyricIndex || !isNewSet)) {
        (function(el) {
          setTimeout(function() {
            el.scrollIntoView({ block: 'center', behavior: 'smooth' });
          }, 50);
        })(line);
      }
    } else {
      newClass += ' future';
    }

    // Só atualiza se a classe mudou (evita reflows desnecessários)
    if (line.className !== newClass) {
      line.className = newClass;
    }
  }
}

// ── Lyric Glide (reajuste suave das letras) ───────────────────────────────────
/**
 * Cancela qualquer glide de letras pendente (timers de passos intermediários).
 */
function cancelLyricGlide() {
  if (state.lyricGlideTimer) {
    clearTimeout(state.lyricGlideTimer);
    state.lyricGlideTimer = null;
  }
}

/**
 * Desliza a linha ativa de `from` até `to`, passo a passo (uma linha por timer),
 * ao longo de até RESYNC_DURATION_MS (mín. 50ms por linha). Usado quando o
 * backend diverge (seek/reconexão) — em vez de pular direto para a linha
 * correta, as linhas intermediárias ficam ativas brevemente, como um glide.
 */
function glideLyricsTo(toIndex) {
  const lyrics = state.lyrics;
  if (!Array.isArray(lyrics) || lyrics.length === 0) return;
  const fromIndex = Math.max(0, Math.min(lyrics.length - 1,
    state.currentLyricIndex >= 0 ? state.currentLyricIndex : 0));
  const clampedTo = Math.max(0, Math.min(lyrics.length - 1, toIndex));
  if (fromIndex === clampedTo) return;

  cancelLyricGlide();

  const step = clampedTo > fromIndex ? 1 : -1;
  const steps = Math.abs(clampedTo - fromIndex);
  // Mínimo 50ms por linha: glide perceptível, sem flash imperceptível
  const perStep = Math.max(50, Math.floor(RESYNC_DURATION_MS / steps));

  let current = fromIndex;
  function next() {
    current += step;
    if ((step > 0 && current >= clampedTo) || (step < 0 && current <= clampedTo)) {
      // Chegou ao destino: render final com scroll normal
      state.currentLyricIndex = clampedTo;
      renderLyrics(lyrics, clampedTo, false, false);
      state.lyricGlideTimer = null;
      return;
    }
    // Linha intermediária: muda a classe ativa sem scroll (evita jitter)
    state.currentLyricIndex = current;
    renderLyrics(lyrics, current, false, true);
    state.lyricGlideTimer = setTimeout(next, perStep);
  }
  next();
}

/**
 * Aplica o índice de linha ativa vindo do backend (lyric-line ou state).
 * Salto pequeno (avanço normal) → direto. Salto grande (divergência) → glide.
 */
function applyLyricIndex(targetIndex) {
  if (!Array.isArray(state.lyrics) || state.lyrics.length === 0) return;
  const fromIdx = state.currentLyricIndex;
  const target = Math.max(0, Math.min(state.lyrics.length - 1, targetIndex));
  if (fromIdx >= 0 && Math.abs(target - fromIdx) > 1) {
    glideLyricsTo(target);
  } else {
    cancelLyricGlide();
    state.currentLyricIndex = target;
    renderLyrics(state.lyrics, target, false, false);
  }
}

/**
 * Rola o painel de letras até a linha ativa — usado após um reajuste suave
 * quando as linhas intermediárias mudaram de classe sem scroll (glide).
 */
function scrollLyricsToActive() {
  const idx = state.currentLyricIndex;
  if (idx < 0 || !els.lyricsList) return;
  const target = els.lyricsList.querySelector('[data-index="' + idx + '"]');
  if (target) target.scrollIntoView({ block: 'center', behavior: 'smooth' });
}

// ── Sync Metrics (seção Sincronia) ────────────────────────────────────────────
/**
 * Atualiza os readouts ao vivo da seção Sincronia:
 *   • Antecipação aplicada (atual) — lead efetivo usado pelo agendador
 *   • Latência da API do Discord — média EWMA medida pelo adaptive sync
 * Chamado a cada state (com syncLeadMs/discordLatencyMs) e no evento
 * leve 'sync-metrics' (quando o adaptive sync reajusta em tempo real).
 */
function updateSyncMetrics() {
  var leadEl = document.getElementById('sync-lead-live');
  if (leadEl) leadEl.textContent = (typeof state.syncLeadMs === 'number' ? state.syncLeadMs : '—') + 'ms';

  var latEl = document.getElementById('sync-latency-live');
  if (latEl) {
    latEl.textContent = (typeof state.discordLatencyMs === 'number' ? state.discordLatencyMs : '—') + 'ms';
    // Destaque visual sutil conforme a latência (verde bom / âmbar médio / vermelho alto)
    var lat = state.discordLatencyMs;
    latEl.classList.remove('lat-good', 'lat-mid', 'lat-high');
    if (lat !== null) {
      latEl.classList.add(lat <= 250 ? 'lat-good' : (lat <= 400 ? 'lat-mid' : 'lat-high'));
    }
  }

  // Modo do adaptive sync: status "Ativo/Manual" + botão de reativar
  var statusEl = document.getElementById('sync-adaptive-status');
  var hintEl = document.getElementById('sync-adaptive-hint');
  var actionsEl = document.getElementById('sync-adaptive-actions');
  if (statusEl) {
    statusEl.textContent = state.adaptiveSyncEnabled ? 'Ativo' : 'Manual';
    statusEl.classList.toggle('sync-auto-active', !!state.adaptiveSyncEnabled);
    statusEl.classList.toggle('sync-auto-manual', !state.adaptiveSyncEnabled);
  }
  if (hintEl) {
    hintEl.textContent = state.adaptiveSyncEnabled
      ? 'O app ajusta a antecipação sozinho conforme a latência da API do Discord.'
      : 'Ajuste automático desativado por um ajuste manual do slider. Reative quando quiser.';
  }
  if (actionsEl) {
    actionsEl.style.display = state.adaptiveSyncEnabled ? 'none' : 'flex';
  }

  // Quando o adaptive sync está ATIVO, o lead exibido é o aplicado pelo app —
  // sincroniza o slider com esse valor (sem disparar o evento 'input', para
  // não reenviar ao backend nem desativar o auto ajuste). Assim a UI nunca
  // mostra o slider em posição divergente do readout "Antecipação aplicada".
  var sliderEl = document.getElementById('sync-lead-slider');
  if (sliderEl && state.adaptiveSyncEnabled && typeof state.syncLeadMs === 'number') {
    if (parseInt(sliderEl.value, 10) !== state.syncLeadMs) {
      sliderEl.value = state.syncLeadMs;
    }
  }
}

// ── Setup Prompt ──────────────────────────────────────────────────────────────
var setupVisible = false;

function showSetupPrompt() {
  if (setupVisible) return;
  setupVisible = true;

  var overlay = document.createElement('div');
  overlay.id = 'setup-overlay';
  overlay.style.cssText = 'position:fixed;inset:0;z-index:100;background:rgba(10,11,14,0.92);display:flex;align-items:center;justify-content:center;backdrop-filter:blur(8px);';

  overlay.innerHTML = '<div style="background:rgba(18,19,23,0.95);border-radius:16px;padding:32px 40px;width:100%;max-width:460px;border:1px solid rgba(255,255,255,0.06);text-align:center;">'
    + '<div style="font-size:48px;margin-bottom:12px;">\uD83C\uDFB5</div>'
    + '<h2 style="color:#e8eaed;font-size:22px;font-weight:700;margin-bottom:4px;">Discord Lyrics Status</h2>'
    + '<p style="color:rgba(232,234,237,0.5);font-size:14px;margin-bottom:24px;">Configure seu token do Discord para come\u00E7ar</p>'
    + '<div style="background:rgba(10,11,14,0.6);border-radius:10px;padding:14px 16px;margin-bottom:20px;font-size:12px;color:rgba(232,234,237,0.6);line-height:2;text-align:left;">'
    + '<strong style="color:#e8eaed;display:block;margin-bottom:4px;">Como obter o User Token:</strong>'
    + '1. Abra discord.com/app no navegador<br>'
    + '2. Pressione F12 - aba Network<br>'
    + '3. Execute qualquer acao<br>'
    + '4. Encontre requisicao - Request Headers - Authorization'
    + '</div>'
    + '<input id="setup-token" type="password" placeholder="Cole o token aqui..." style="width:100%;padding:11px 14px;background:rgba(10,11,14,0.8);border:1.5px solid rgba(255,255,255,0.08);border-radius:8px;color:#e8eaed;font-size:14px;outline:none;margin-bottom:4px;font-family:inherit;">'
    + '<div id="setup-error" style="color:#f23f42;font-size:13px;margin-top:8px;min-height:18px;"></div>'
    + '<button id="setup-btn" style="margin-top:14px;width:100%;padding:13px;background:#8b5cf6;border:none;border-radius:8px;color:#fff;font-size:15px;font-weight:700;cursor:pointer;font-family:inherit;">'
    + 'Salvar e iniciar</button>'
    + '<p style="margin-top:12px;font-size:12px;color:rgba(232,234,237,0.3);">Token armazenado localmente na sua maquina.</p>'
    + '</div>';

  document.body.appendChild(overlay);

  document.getElementById('setup-btn').addEventListener('click', setupSave);
  document.getElementById('setup-token').addEventListener('keydown', function(e) {
    if (e.key === 'Enter') setupSave();
  });
}

function hideSetupPrompt() {
  if (!setupVisible) return;
  var overlay = document.getElementById('setup-overlay');
  if (overlay) overlay.remove();
  setupVisible = false;
}

async function setupSave() {
  var token = document.getElementById('setup-token').value.trim();
  var errorEl = document.getElementById('setup-error');
  var btn = document.getElementById('setup-btn');

  if (!token) {
    errorEl.textContent = 'Por favor, insira o token.';
    return;
  }

  btn.disabled = true;
  btn.textContent = 'Verificando...';
  errorEl.textContent = '';

  try {
    const res = await fetch('/api/setup', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token }),
    });
    const data = await res.json();

    if (data.ok) {
      state.tokenConfigured = true;
      state.discordAccountName = data.username || null;
      state.discordAccountTag = data.tag || null;
      state.discordAccountAvatarUrl = data.avatarUrl || null;
      updateDiscordAccountUI();
      addLog('success', 'SETUP', 'Logged in as ' + (data.username || data.tag || 'user'));
      addLog('success', 'APP', 'Token configured. Starting monitor.');
      hideSetupPrompt();
      // Busca estado atualizado
      const stateRes = await fetch('/api/status');
      const newState = await stateRes.json();
      updateUI(newState);
    } else {
      errorEl.textContent = data.error || 'Error validating token.';
      btn.disabled = false;
      btn.textContent = 'Salvar e iniciar';
    }
  } catch (err) {
    errorEl.textContent = 'Erro ao conectar com o servidor.';
    btn.disabled = false;
    btn.textContent = 'Salvar e iniciar';
  }
}

// ── WebSocket Connection ──────────────────────────────────────────────────────
/** @type {WebSocket|null} */
let ws = null;
/** @type {number|null} */
let wsReconnectTimer = null;
/** @type {number} */
let wsReconnectAttempts = 0;
const WS_MAX_RECONNECT_MS = 10000;
const WS_BASE_RECONNECT_MS = 1000;
/** Drift (ms) entre o progresso local interpolado e o backend que dispara o reajuste suave */
const RESYNC_THRESHOLD_MS = 1200;
/**
 * Tolerância (ms) para o backend estar ATRÁS do relógio local sem reconciliar.
 * O daemon SMTC quantiza a posição em segundos inteiros e pode atrasar 1–3s na
 * troca de faixa. Puxar a barra/anel de volta a cada broadcast travava o anel
 * em ~0% no início da música (glide repetido para o valor atrasado). Só
 * reconcilia para trás em saltos grandes de verdade (seek externo, restart).
 */
const RESYNC_BACKWARD_TOLERANCE_MS = 3000;
/** Duração (ms) do glide de reconciliação da barra de progresso */
const RESYNC_DURATION_MS = 500;

/**
 * Conecta ao servidor via WebSocket para receber eventos em tempo real.
 * Reconexão automática com exponential backoff (1s → 2s → 4s → 8s → 10s max).
 *
 * Idempotente: se já existe uma conexão aberta ou em andamento, não cria outra.
 * Cada socket é rastreado por identidade — onclose/onerror de um socket ANTIGO
 * (fechado no startup ou numa reconexão) nunca pode derrubar a referência (ws)
 * de um socket mais novo. Isso elimina o churn close→reconnect do startup.
 */
function connectWebSocket() {
  // Já existe conexão viva ou em handshake — não duplica
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) {
    DBG.log('WS', 'connectWebSocket() skipped — connection already active');
    return;
  }

  DBG.log('WS', `connectWebSocket() called (attempt ${wsReconnectAttempts + 1})`);

  // Fecha socket morto pendente (CLOSED/CLOSING), se houver, antes de recriar
  if (ws) {
    try { ws.close(); } catch {}
    ws = null;
  }

  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const wsUrl = protocol + '//' + window.location.host + '/api/ws';
  DBG.log('WS', `Connecting to ${wsUrl}`);

  let socket;
  try {
    socket = new WebSocket(wsUrl);
  } catch (err) {
    console.error('[WS] Failed to create WebSocket:', err);
    scheduleWsReconnect();
    if (typeof updateWsFooter === 'function') updateWsFooter();
    updateProgressSyncStatus();
    updateDiagBanner();
    return;
  }
  ws = socket;

  // Estado intermediário "Conectando..." enquanto o handshake acontece
  if (typeof updateWsFooter === 'function') updateWsFooter();

  socket.onopen = function() {
    if (ws !== socket) return; // socket substituído — ignora (race do startup)
    DBG.log('WS', `Connected to server (attempt ${wsReconnectAttempts + 1})`);
    wsReconnectAttempts = 0;
    if (wsReconnectTimer) {
      clearTimeout(wsReconnectTimer);
      wsReconnectTimer = null;
    }
    // Atualiza indicadores visuais
    _diagData.updateDot();
    if (typeof updateWsFooter === 'function') updateWsFooter();
    if (typeof updateDiagPanel === 'function') updateDiagPanel();
    updateProgressSyncStatus();
    updateDiagBanner();
  };

  socket.onmessage = function(e) {
    if (ws !== socket) return; // mensagem de socket antigo — descarta
    const rawSize = e.data.length;
    _diagData.wsLastMsgTime = Date.now();
    _diagData.wsMsgCount++;
    try {
      const msg = JSON.parse(e.data);
      DBG.log('WS', `Message #${_diagData.wsMsgCount} received: type=${msg.type}, size=${rawSize} bytes`);
      handleWsMessage(msg);
    } catch (err) {
      console.error('[WS] Failed to parse message:', err);
    }
  };

  socket.onclose = function(e) {
    // Guard crítico: onclose de um socket antigo (fechado no startup ou numa
    // reconexão) NÃO pode anular o socket atual nem agendar nova reconexão.
    if (ws !== socket) return;
    DBG.log('WS', `Connection closed: code=${e?.code}, reason=${e?.reason || '(none)'}, wasClean=${e?.wasClean}`);
    ws = null;
    scheduleWsReconnect();
    _diagData.updateDot();
    if (typeof updateWsFooter === 'function') updateWsFooter();
    updateProgressSyncStatus();
    updateDiagBanner();
  };

  socket.onerror = function() {
    DBG.error('WS', 'WebSocket connection error (no detail available)');
    // O navegador dispara onclose após onerror em falhas de conexão; se por
    // algum motivo não vier, força o fechamento para o ciclo de reconexão rodar.
    if (ws === socket && socket.readyState !== WebSocket.OPEN) {
      try { socket.close(); } catch {}
    }
    updateProgressSyncStatus();
    updateDiagBanner();
  };
}

/**
 * Dispara uma mensagem WebSocket para o handler apropriado baseado no tipo.
 * @param {{type: string, data: *}} msg
 */
function handleWsMessage(msg) {
  DBG.log('WS', `Handle msg type=${msg.type}`, msg.data ? Object.keys(msg.data).slice(0, 8) : '(no data)');

  switch (msg.type) {
    case 'state':
      DBG.log('STATE', `Track=${msg.data?.trackName || '(none)'}, playing=${msg.data?.isPlaying}, lyricsState=${msg.data?.lyricsState}`);
      updateUI(msg.data);
      break;

    case 'log':
      if (msg.data && msg.data.tag && msg.data.message) {
        addLog('info', msg.data.tag, msg.data.message);
      }
      break;

    case 'lyric-line':
      DBG.log('LYRICS', `Line index=${msg.data?.index}, total=${state.lyrics.length}`);
      if (msg.data && msg.data.index !== undefined && state.lyrics.length > 0) {
        // NÃO atualiza progressMs/lastProgressTime aqui — o RAF (requestAnimationFrame)
        // é responsável pela interpolação suave da barra de progresso.
        // O LyricScheduler do backend é a única fonte para avanço de letras sincronizadas.
        // Saltos grandes (seek/divergência) deslizam pelas linhas intermediárias.
        applyLyricIndex(msg.data.index);
      }
      break;

    case 'sync-metrics':
      // Adaptive sync: atualiza lead/latência em tempo real (evento leve, sem state completo)
      DBG.log('SYNC', `Lead=${msg.data?.leadMs}ms, latency=${msg.data?.latencyMs}ms`);
      if (msg.data && typeof msg.data.leadMs === 'number') {
        state.syncLeadMs = msg.data.leadMs;
      }
      if (msg.data && typeof msg.data.latencyMs === 'number') {
        state.discordLatencyMs = msg.data.latencyMs;
      }
      if (msg.data && typeof msg.data.adaptiveSyncEnabled === 'boolean') {
        state.adaptiveSyncEnabled = msg.data.adaptiveSyncEnabled;
      }
      updateSyncMetrics();
      break;

    case 'status-changed':
      if (msg.data.status === 'setup-complete' || msg.data.status === 'ready') {
        addLog('success', 'APP', 'Status: ' + msg.data.status);
      } else if (msg.data.status === 'setup-needed') {
        addLog('warn', 'APP', 'Token configuration required');
      }
      break;
  }
}

/**
 * Agenda reconexão com exponential backoff.
 * Guard: se já há uma conexão viva/em andamento, não agenda (evita timers
 * fantasma que fechariam a conexão atual).
 */
function scheduleWsReconnect() {
  if (wsReconnectTimer) {
    clearTimeout(wsReconnectTimer);
    wsReconnectTimer = null;
  }
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) {
    DBG.log('WS', 'scheduleWsReconnect() skipped — connection already active');
    return;
  }
  wsReconnectAttempts++;
  const delay = Math.min(
    WS_BASE_RECONNECT_MS * Math.pow(2, wsReconnectAttempts - 1),
    WS_MAX_RECONNECT_MS
  );
  DBG.log('WS', `Reconnecting in ${delay}ms (attempt ${wsReconnectAttempts})`);
  wsReconnectTimer = setTimeout(connectWebSocket, delay);
}

// ── UI Interactions ───────────────────────────────────────────────────────────

/**
 * Abre o drawer de configurações.
 */
function openSettings() {
  // Fecha diagnóstico se estiver aberto
  const diagDrawer = document.getElementById('diag-drawer');
  if (diagDrawer && diagDrawer.classList.contains('open')) closeDiag();

  const overlay = document.getElementById('settings-overlay');
  const drawer = document.getElementById('settings-drawer');
  if (overlay) overlay.classList.add('open');
  if (drawer) drawer.classList.add('open');
  document.body.style.overflow = 'hidden';
}

/**
 * Fecha o drawer de configurações.
 */
function closeSettings() {
  const overlay = document.getElementById('settings-overlay');
  const drawer = document.getElementById('settings-drawer');
  if (overlay) overlay.classList.remove('open');
  if (drawer) drawer.classList.remove('open');
  document.body.style.overflow = '';
}

// ── Diagnostics Panel ────────────────────────────────────────────────────────
/** Dados de diagnóstico para o painel */
const _diagData = {
  wsLastMsgTime: 0,
  wsMsgCount: 0,
  diagRefreshTimer: null,
  /** Atualiza o dot do botão de diagnóstico baseado no estado do WS */
  updateDot() {
    const dot = document.getElementById('diag-dot');
    if (!dot) return;
    if (ws && ws.readyState === WebSocket.OPEN) {
      dot.setAttribute('data-ws', 'connected');
    } else if (ws && ws.readyState === WebSocket.CONNECTING) {
      dot.setAttribute('data-ws', 'checking');
    } else {
      dot.setAttribute('data-ws', 'disconnected');
    }
  },
};

/** Abre o drawer de diagnóstico */
function openDiag() {
  const overlay = document.getElementById('diag-overlay');
  const drawer = document.getElementById('diag-drawer');
  if (overlay) overlay.classList.add('open');
  if (drawer) drawer.classList.add('open');
  updateDiagPanel();
  // Auto-refresh a cada 500ms enquanto o drawer estiver aberto
  if (_diagData.diagRefreshTimer) clearInterval(_diagData.diagRefreshTimer);
  _diagData.diagRefreshTimer = setInterval(updateDiagPanel, 500);
}

/** Fecha o drawer de diagnóstico */
function closeDiag() {
  const overlay = document.getElementById('diag-overlay');
  const drawer = document.getElementById('diag-drawer');
  if (overlay) overlay.classList.remove('open');
  if (drawer) drawer.classList.remove('open');
  if (_diagData.diagRefreshTimer) {
    clearInterval(_diagData.diagRefreshTimer);
    _diagData.diagRefreshTimer = null;
  }
}

/** Atualiza todos os campos do painel de diagnóstico */
function updateDiagPanel() {
  const $id = (id) => document.getElementById(id);

  // Helper para setar valor + data-status em um diag-item
  function setDiagVal(id, val, status) {
    const el = $id(id);
    if (!el) return;
    el.textContent = val ?? '---';
    if (status !== undefined) el.setAttribute('data-status', String(status));
    else el.removeAttribute('data-status');
  }

  // ── WebSocket Status ─────────────────────────────────────────
  const wsConnected = ws && ws.readyState === WebSocket.OPEN;
  const wsChecking = wsReconnectAttempts > 0;

  if (wsConnected) {
    setDiagVal('diag-ws-status', 'Conectado', 'connected');
  } else if (wsChecking) {
    // Limita o número exibido no painel (backoff já está limitado a 10s)
    const shown = Math.min(wsReconnectAttempts, 999);
    setDiagVal('diag-ws-status', `Reconectando (${shown})`, 'checking');
  } else {
    setDiagVal('diag-ws-status', 'Desconectado', 'disconnected');
  }

  setDiagVal('diag-ws-attempts', String(Math.min(wsReconnectAttempts, 999)));

  // Última msg
  if (_diagData.wsLastMsgTime > 0) {
    const lastTime = new Date(_diagData.wsLastMsgTime).toLocaleTimeString('pt-BR');
    setDiagVal('diag-ws-last', lastTime);

    // Idade da última mensagem
    const ageMs = Date.now() - _diagData.wsLastMsgTime;
    if (ageMs < 1000) setDiagVal('diag-ws-age', 'agora');
    else if (ageMs < 60000) setDiagVal('diag-ws-age', `${Math.floor(ageMs / 1000)}s`);
    else setDiagVal('diag-ws-age', `${Math.floor(ageMs / 60000)}min`);
  } else {
    setDiagVal('diag-ws-last', '---');
    setDiagVal('diag-ws-age', '---');
  }

  // ── Track Info ───────────────────────────────────────────────
  setDiagVal('diag-track', state.trackName || 'Nenhuma');
  setDiagVal('diag-artist', state.artistName || '---');

  if (state.isPlaying) {
    setDiagVal('diag-playing', 'Sim', 'playing');
  } else if (state.trackName) {
    setDiagVal('diag-playing', 'Pausado', 'paused');
  } else {
    setDiagVal('diag-playing', 'Não', 'false');
  }

  setDiagVal('diag-platform', state.platform && state.platform !== 'unknown' ? state.platform.toUpperCase() : '---');
  setDiagVal('diag-progress', formatTime(getCurrentProgressMs()));
  setDiagVal('diag-duration', formatTime(state.durationMs));

  // ── Lyrics Status ────────────────────────────────────────────
  const lyricStateLabels = {
    'active': { label: 'Ativo', status: 'active' },
    'loading': { label: 'Carregando', status: 'loading' },
    'empty': { label: 'Vazio', status: 'idle' },
    'idle': { label: 'Ocioso', status: 'idle' },
  };
  const ls = lyricStateLabels[state.lyricsState] || { label: state.lyricsState, status: 'idle' };
  setDiagVal('diag-lyrics-state', ls.label, ls.status);
  setDiagVal('diag-lyrics-count', String(state.lyricsCount || 0));
  setDiagVal('diag-lyrics-idx', String(state.currentLyricIndex));
  setDiagVal('diag-lyrics-synced', state.lyricsSynced ? 'Sim' : 'Não', String(state.lyricsSynced));
  setDiagVal('diag-lyrics-ver', String(state.lyricsVersion || 0));
  setDiagVal('diag-lyrics-src', state.lyricsSynced ? 'LRCLIB' : (state.lyricsState === 'active' ? 'Genius' : '---'));

  // ── Connection Status ────────────────────────────────────────
  setDiagVal('diag-conn-discord', state.discordStatus === 'connected' ? 'Conectado' : state.discordStatus === 'disconnected' ? 'Desconectado' : 'Verificando', state.discordStatus);
  setDiagVal('diag-conn-smtc', state.smtcStatus === 'connected' ? 'Conectado' : state.smtcStatus === 'disconnected' ? 'Desconectado' : 'Verificando', state.smtcStatus);
  setDiagVal('diag-conn-lyrics', state.lyricsSourceStatus === 'connected' ? 'Conectado' : state.lyricsSourceStatus === 'disconnected' ? 'Desconectado' : 'Verificando', state.lyricsSourceStatus);
  setDiagVal('diag-conn-token', state.tokenConfigured ? 'Configurado' : 'Não configurado', String(state.tokenConfigured));

  // ── Footer: next update countdown ────────────────────────────
  const nextUp = $id('diag-next-update');
  if (nextUp) {
    const remaining = 500 - (performance.now() % 500);
    nextUp.textContent = (remaining / 1000).toFixed(1) + 's';
  }

  // Update the diagnostic dot in header
  _diagData.updateDot();
}

// ── Volume and Preview functions removed ────────────────────────────────

// ── Progress Bar Interactions ────────────────────────────────────────────────

// ── Progress Bar (unificada — transform-based, sem tremor) ───────────────────
// Barra, thumb e hover preview são dirigidos por custom properties
// (--progress e --hover-progress) aplicadas em #progress-bar-bg. O CSS usa
// transform: scaleX() (GPU) — zero layout por frame e zero transição de width.
// Todo o input (clique/drag/touch) vive em "Seek: Click/Drag" mais abaixo.

/** Largura do waveform em px de dispositivo (cache — não ler layout no hot path). */
let _progressBarWidthPx = 0;

/**
 * Atualiza o cache de largura do waveform (chamado no init, resize e troca de faixa).
 * Usado para quantizar o progresso em pixels inteiros — a borda lit/unlit do
 * clip-path deixa de cair em posições fracionárias, eliminando o tremor de
 * subpixel da camada composta (will-change: clip-path re-rasteriza a borda).
 */
function refreshProgressBarWidth() {
  if (els.progressBarBg) {
    const rect = els.progressBarBg.getBoundingClientRect();
    _progressBarWidthPx = Math.max(1, Math.round(rect.width * (window.devicePixelRatio || 1)));
  }
}

/**
 * Quantiza uma fração de progresso (0..1) para um múltiplo de 1px de dispositivo.
 * Sem isso, valores como 0.6324611610793132 colocam a borda do clip-path entre
 * dois pixels e a borda composta treme a cada frame.
 * @param {number} frac 0..1
 * @returns {number} fração quantizada (sem mudança se a largura não for conhecida)
 */
function snapProgressFraction(frac) {
  if (_progressBarWidthPx <= 0) return frac;
  return Math.round(frac * _progressBarWidthPx) / _progressBarWidthPx;
}

/**
 * Define o progresso (0..100) da barra via custom property.
 * Quantiza em pixels inteiros de dispositivo para a borda do clip-path
 * nunca cair entre dois pixels (elimina o tremor de subpixel).
 * @param {number} pct 0..100
 */
function setProgressPct(pct) {
  if (!els.progressBarBg) return;
  const c = Math.max(0, Math.min(100, pct));
  els.progressBarBg.style.setProperty('--progress', String(snapProgressFraction(c / 100)));
}

// ── Waveform Seek (barras verticais no lugar da barra de progresso) ──────────
/** Número de barras do waveform (custo fixo, ~44 nós × 3 camadas) */
const WAVEFORM_BARS = 44;

/**
 * Gera o padrão de barras do waveform (alturas pseudo-aleatórias por faixa).
 * A mesma seed (nome da música) produz o MESMO padrão a cada reconstrução,
 * então o waveform é estável enquanto a faixa não muda — e muda de "forma"
 * ao trocar de música. O progresso NÃO mexe no DOM: as camadas lit/preview
 * são recortadas via clip-path (--progress / --hover-progress) no CSS.
 * @param {string} seed Texto usado como semente do padrão (ex: nome da faixa)
 */
function buildWaveform(seed) {
  if (!els.waveformBars) return;
  const s = String(seed || 'default');
  // PRNG determinístico (mulberry32) a partir da seed
  let h = 1779033703 ^ (s.length * 31);
  for (let i = 0; i < s.length; i++) {
    h = Math.imul(h ^ s.charCodeAt(i), 3432918353);
    h = (h << 13) | (h >>> 19);
  }
  function rand() {
    h |= 0; h = (h + 0x6D2B79F5) | 0;
    let t = Math.imul(h ^ (h >>> 15), 1 | h);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  }
  // Envelope senoidal (barras baixas nas pontas, altas no meio) + ruído
  let html = '';
  for (let i = 0; i < WAVEFORM_BARS; i++) {
    const env = Math.sin((i / (WAVEFORM_BARS - 1)) * Math.PI);
    const pct = Math.round(18 + 76 * (0.35 + 0.65 * rand()) * (0.5 + 0.5 * env));
    html += '<div class="waveform-bar" style="--h:' + pct + '%"></div>';
  }
  els.waveformBars.innerHTML = html;
  if (els.waveformLit) els.waveformLit.innerHTML = html;
  if (els.waveformPreview) els.waveformPreview.innerHTML = html;
  refreshProgressBarWidth();
}

/**
 * Define o hover preview (0..100) via custom property.
 * @param {number} pct 0..100
 */
function setHoverPct(pct) {
  if (!els.progressBarBg) return;
  const c = Math.max(0, Math.min(100, pct));
  els.progressBarBg.style.setProperty('--hover-progress', String(snapProgressFraction(c / 100)));
}

/**
 * Atualiza o status de sincronização da barra de progresso (#progress-sync-status):
 * - WS desconectado/reconectando → "Reconectando…" (laranja)
 * - Progresso local divergindo   → "Sincronizando…" (accent)
 * - Nenhum dos dois              → escondido
 */
function updateProgressSyncStatus() {
  const el = document.getElementById('progress-sync-status');
  if (!el) return;
  const wsOpen = !!(ws && ws.readyState === WebSocket.OPEN);
  let mode = null;
  if (state.durationMs > 0 && state.trackName) {
    if (!wsOpen) mode = 'reconnecting';
    else if (state.resyncActive) mode = 'syncing';
  }
  el.classList.toggle('visible', !!mode);
  el.classList.toggle('reconnecting', mode === 'reconnecting');
  el.classList.toggle('syncing', mode === 'syncing');
  const label = el.querySelector('.pss-label');
  if (label) label.textContent = mode === 'reconnecting' ? 'Reconectando…' : 'Sincronizando…';
}

// ── Diagnostic Banner (auto-diagnóstico no topo da UI) ───────────────────────
/** Modo do banner que o usuário dispensou — re-mostra apenas quando o modo mudar */
let _bannerDismissedMode = null;

const DIAG_BANNER_MODES = {
  backend: 'Backend offline — inicie o app',
  token:   'Token do Discord não configurado',
  smtc:    'Spotify desconectado — verifique o daemon SMTC',
  paused:  'Spotify pausado',
};

/**
 * Atualiza o banner de auto-diagnóstico no topo da UI. Prioridade:
 *   backend (WS caiu/reconectando) > token > smtc desconectado > pausado.
 */
function updateDiagBanner() {
  const el = document.getElementById('diag-banner');
  if (!el) return;

  let mode = null;
  if (wsReconnectAttempts > 0 || (ws && ws.readyState === WebSocket.CLOSED)) {
    mode = 'backend';
  } else if (state._stateReceived) {
    // Só avalia os modos que dependem do estado do backend após o primeiro
    // broadcast (state._stateReceived) — evita flash de "Token não configurado"
    // no carregamento quando o token JÁ está salvo. O backend envia
    // tokenConfigured de verdade; appReady fica redundante aqui.
    if (state.tokenConfigured === false) {
      mode = 'token';
    } else if (state.smtcStatus === 'disconnected') {
      mode = 'smtc';
    } else if (state.trackName && state.artistName && !state.isPlaying) {
      mode = 'paused';
    }
  }

  const show = !!mode && mode !== _bannerDismissedMode;
  el.classList.toggle('visible', show);
  // DRY: itera as chaves do mapa de modos (Object.keys preserva a ordem)
  Object.keys(DIAG_BANNER_MODES).forEach(function(m) {
    el.classList.toggle('mode-' + m, mode === m);
  });
  const textEl = document.getElementById('diag-banner-text');
  if (textEl && mode) textEl.textContent = DIAG_BANNER_MODES[mode];
}

// Dismiss manual: esconde até o MODO do banner mudar
(function() {
  const closeBtn = document.getElementById('diag-banner-close');
  if (!closeBtn) return;
  closeBtn.addEventListener('click', function() {
    const el = document.getElementById('diag-banner');
    _bannerDismissedMode = null;
    if (el) {
      Object.keys(DIAG_BANNER_MODES).forEach(function(m) {
        if (el.classList.contains('mode-' + m)) _bannerDismissedMode = m;
      });
      el.classList.remove('visible');
    }
  });
})();

/**
 * Envia seek para o servidor com base na posição do clique na barra.
 * (Atalhos de teclado ◀ ▶ chamam esta função — delega ao fluxo unificado.)
 */
function seekProgress(clientX) {
  if (!els.progressBarBg || state.durationMs <= 0) return;
  const rect = els.progressBarBg.getBoundingClientRect();
  const fraction = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
  doSeek(fraction, true);
}


// ── Keyboard Shortcuts ────────────────────────────────────────────────────────
/**
 * Atalhos de teclado:
 *   →    → Avançar 5s
 *   ←    → Voltar 5s
 *   Esc  → Fechar config / diagnóstico
 */
document.addEventListener('keydown', function(e) {
  // Ignora se estiver digitando em inputs
  if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') return;

  // Escape: fecha settings drawer (se aberto)
  if (e.key === 'Escape') {
    closeSettings();
    return;
  }

  // Se o drawer de configurações estiver aberto, ignora outros atalhos
  const drawer = document.getElementById('settings-drawer');
  if (drawer && drawer.classList.contains('open')) return;

  switch (e.code) {
    case 'ArrowRight':
      e.preventDefault();
      // Seek forward 5s
      if (state.durationMs > 0) {
        var newMs = Math.min(state.durationMs, getCurrentProgressMs() + 5000);
        var rect = els.progressBarBg ? els.progressBarBg.getBoundingClientRect() : null;
        if (rect) seekProgress(rect.left + (newMs / state.durationMs) * rect.width);
      }
      break;

    case 'ArrowLeft':
      e.preventDefault();
      // Seek backward 5s
      if (state.durationMs > 0) {
        var newMs = Math.max(0, getCurrentProgressMs() - 5000);
        var rect = els.progressBarBg ? els.progressBarBg.getBoundingClientRect() : null;
        if (rect) seekProgress(rect.left + (newMs / state.durationMs) * rect.width);
      }
      break;
  }
});

// ── UI Event Listeners ─────────────────────────────────────────────────────────
document.addEventListener('DOMContentLoaded', function() {
  DBG.log('UI', 'DOMContentLoaded fired — initializing UI');
  DBG.log('UI', `Found DOM elements: trackName=${!!els.trackName}, lyricsList=${!!els.lyricsList}, progressBar=${!!els.progressBarBg}, equalizer=${!!els.equalizer}`);

  // Waveform seek: gera as barras iniciais (padrão neutro até chegar uma faixa)
  buildWaveform('default');
  // Cache da largura do waveform — usado para quantizar --progress em pixels
  // inteiros (anti-tremor de subpixel do clip-path).
  refreshProgressBarWidth();
  window.addEventListener('resize', function() {
    refreshProgressBarWidth();
  });

  // ── Diagnostics drawer ──────────────────────────────────────
  const diagBtn = document.getElementById('diag-btn');
  if (diagBtn) {
    diagBtn.addEventListener('click', function(e) {
      e.stopPropagation();
      // Se settings estiver aberto, fecha primeiro
      const settingsDrawer = document.getElementById('settings-drawer');
      if (settingsDrawer && settingsDrawer.classList.contains('open')) closeSettings();
      openDiag();
    });
  }

  // ── Fullscreen toggle ──────────────────────────────────────────────
  const fsBtn = document.getElementById('fs-btn');
  let _fsMode = false;

  function _setFsMode(on) {
    _fsMode = !!on;
    document.body.classList.toggle('fs-mode', _fsMode);
    if (fsBtn) {
      fsBtn.classList.toggle('active', _fsMode);
      fsBtn.setAttribute('title', _fsMode ? 'Sair da tela cheia (F11)' : 'Tela cheia (F11)');
    }
  }

  function _toggleFullscreen() {
    if (window.electronAPI && window.electronAPI.toggleFullscreen) {
      // Electron: usa a janela nativa (F11 também cai aqui via atalho abaixo)
      window.electronAPI.toggleFullscreen();
    } else if (document.documentElement.requestFullscreen) {
      // Fallback browser/dev: Fullscreen API
      if (document.fullscreenElement) {
        document.exitFullscreen().catch(function() {});
      } else {
        document.documentElement.requestFullscreen().catch(function() {});
      }
    }
  }

  if (fsBtn) {
    fsBtn.addEventListener('click', _toggleFullscreen);
  }

  // Sincroniza com eventos nativos (Electron) e da Fullscreen API (browser)
  if (window.electronAPI && window.electronAPI.onFullscreenChange) {
    window.electronAPI.onFullscreenChange(_setFsMode);
  }
  document.addEventListener('fullscreenchange', function() {
    _setFsMode(!!document.fullscreenElement);
    // O layout pode mudar sem window resize (modo tela cheia) — atualiza o
    // cache de largura do waveform para o snap de pixels continuar correto.
    refreshProgressBarWidth();
  });

  // Atalho F11 — só no Electron. No browser o F11 é nativo do navegador e
  // interceptá-lo causaria double-toggle (entra e sai na mesma tecla); lá o
  // listener de fullscreenchange acima já sincroniza a classe fs-mode.
  if (window.electronAPI && window.electronAPI.toggleFullscreen) {
    document.addEventListener('keydown', function(e) {
      if (e.key === 'F11' && e.target.tagName !== 'INPUT' && e.target.tagName !== 'TEXTAREA') {
        e.preventDefault();
        _toggleFullscreen();
      }
    });
  }

  const diagClose = document.getElementById('diag-close');
  if (diagClose) {
    diagClose.addEventListener('click', closeDiag);
  }

  const diagOverlay = document.getElementById('diag-overlay');
  if (diagOverlay) {
    diagOverlay.addEventListener('click', closeDiag);
  }

  const diagRefresh = document.getElementById('diag-refresh');
  if (diagRefresh) {
    diagRefresh.addEventListener('click', function() {
      updateDiagPanel();
      // Feedback visual no botão
      this.querySelector('svg').style.transform = 'rotate(360deg)';
      setTimeout(() => { this.querySelector('svg').style.transform = ''; }, 400);
    });
  }

  // Fechar diag com Escape
  document.addEventListener('keydown', function(e) {
    if (e.key === 'Escape') {
      const diagDrawer = document.getElementById('diag-drawer');
      if (diagDrawer && diagDrawer.classList.contains('open')) {
        closeDiag();
      }
    }
  });

  // Inicializar dot do diagnóstico
  _diagData.updateDot();
  if (typeof updateConnectionsSummary === 'function') updateConnectionsSummary();
  // updateWsFooter é acionado por ws.onopen/onclose — aqui o WS ainda não conectou

  // ── Equalizer: ativa animação quando música está tocando ────────
  function updateEqualizer() {
    if (!els.equalizer) return;
    if (state.isPlaying) {
      els.equalizer.classList.add('active');
      els.equalizer.classList.remove('paused');
    } else if (state.trackName) {
      // Música pausada - equalizador com animação sutil
      els.equalizer.classList.add('active');
      els.equalizer.classList.add('paused');
    } else {
      els.equalizer.classList.remove('active', 'paused');
    }
  }

  // Patch updateUI to also update equalizer state
  var _origUpdateUI = updateUI;
  updateUI = function(data) {
    _origUpdateUI(data);
    updateEqualizer();
  };

  // Sync lead slider
  const syncLeadSlider = document.getElementById('sync-lead-slider');
  const syncLeadValue = document.getElementById('sync-lead-value');
  if (syncLeadSlider && syncLeadValue) {
    syncLeadSlider.addEventListener('input', function() {
      syncLeadValue.textContent = this.value + 'ms';
    });
    syncLeadSlider.addEventListener('change', function() {
      fetch('/api/settings', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ syncLeadMs: parseInt(this.value, 10) }),
      }).catch(function(err) {
        console.error('[Settings] Failed to save sync lead:', err);
      });
    });
  }

  // Theme cards in settings — ripple visual (a aplicação do tema fica com o
  // handler delegado em settingsThemeGrid)
  document.querySelectorAll('.theme-card').forEach(function(card) {
    card.addEventListener('click', function(e) {
      // ── Ripple effect at click position (visual only — a aplicação
      // do tema é feita pelo handler delegado em settingsThemeGrid) ──
      var oldRipple = this.querySelector('.ripple');
      if (oldRipple) oldRipple.remove();

      var ripple = document.createElement('span');
      ripple.className = 'ripple';

      var rect = this.getBoundingClientRect();
      var size = Math.max(rect.width, rect.height) * 2.2;
      var x = e.clientX - rect.left - size / 2;
      var y = e.clientY - rect.top - size / 2;

      ripple.style.width = size + 'px';
      ripple.style.height = size + 'px';
      ripple.style.left = x + 'px';
      ripple.style.top = y + 'px';

      // Cor combina com o card (não com o tema ativo)
      var themeId = this.getAttribute('data-theme-id');
      var cardColors = {
        purple: 'rgba(139, 92, 246, 0.30)',
        green:  'rgba(29, 185, 84, 0.30)',
        blue:   'rgba(88, 101, 242, 0.30)',
        orange: 'rgba(249, 115, 22, 0.30)',
        pink:   'rgba(233, 30, 138, 0.30)',
        teal:   'rgba(13, 148, 136, 0.30)',
        amber:  'rgba(217, 119, 6, 0.30)',
        rose:   'rgba(225, 29, 72, 0.30)',
        indigo: 'rgba(79, 70, 229, 0.30)',
      };
      ripple.style.background = cardColors[themeId] || 'rgba(255, 255, 255, 0.12)';

      this.appendChild(ripple);
      setTimeout(function() {
        if (ripple.parentNode) ripple.remove();
      }, 700);
    });
  });

  // Log clear
  if (els.logClear) {
    els.logClear.addEventListener('click', function() {
      els.logContainer.innerHTML = '<div class="log-empty"><div class="log-empty-icon"><svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg></div><span>App activity will appear here...</span></div>';
      updateLogCount();
    });
  }

  // Desempenho: aplica efeitos de fundo / modo econômico salvos ANTES
  // de iniciar as partículas (fx-off não deve spawnar partículas)
  applySavedPerfSettings();
  initOptimizationSettings();
  initEmoteSettings();
  initTokenSettings();

  // Initialize particles (respeita a preferência de efeitos de fundo)
  if (!document.body.classList.contains('fx-off')) {
    particleSystem.init();
    particleSystem.start();
    // Eco mode: não spawna novas partículas mesmo com fx ligado (o CSS
    // esconderia, mas o timer continuaria criando nós no DOM oculto)
    if (document.body.classList.contains('eco-mode')) {
      particleSystem.pause();
    }
  }

});

// ── Local Progress Interpolation ──────────────────────────────────────────────
/** Flag para pausar RAF quando a aba está oculta (economiza CPU/GPU) */
let _pageHidden = false;

/**
 * Retorna o progresso interpolado atual (com base no tempo decorrido desde
 * a última atualização do WebSocket). Usado pelos atalhos de teclado (seek).
 * @returns {number} Progresso em ms
 */
function getCurrentProgressMs() {
  if (!state.isPlaying || state.durationMs <= 0) return state.progressMs || 0;
  const elapsed = performance.now() - state.lastProgressTime;
  return Math.min(state.progressMs + elapsed, state.durationMs);
}


/**
 * Sincroniza o arco do anel da capa com o progresso real da faixa.
 * @param {number} pct Progresso em percentual (0..100)
 */
function setArtRingProgress(pct) {
  if (!els.albumArt) return;
  const clamped = Math.max(0, Math.min(100, pct));
  // Quantiza a fração em passos de 1/1000 (0.36° do arco de 360°): invisível,
  // mas evita micro-jitter da ponta do anel (conic-gradient re-amostra por frame).
  const snapped = Math.round((clamped / 100) * 1000) / 1000;
  els.albumArt.style.setProperty('--ring-progress', String(snapped));
}

/** Faz a barra de progresso e o timer andarem suavemente entre SSE events */
function startProgressAnimation() {
  // Force-restart: cancela qualquer frame pendente (mesmo um ID stale de um
  // loop que morreu por exceção) para nunca travar barra/anel num estado morto.
  if (state.progressAnimFrame) {
    cancelAnimationFrame(state.progressAnimFrame);
    state.progressAnimFrame = null;
  }

  // Cache de variáveis para reduzir acesso a objeto no hot path
  let _lyrics = state.lyrics;
  let _synced = state.lyricsSynced;

  // Contador de frames para throttle: só atualiza UI a cada 2 frames (~50fps → ~25fps)
  var _tickFrameCount = 0;
  // `_lastTimeText` é module-level (compartilhado com doSeek/pause/reset)
  // Erros consecutivos: após N, para o loop e deixa o watchdog reiniciar (1s)
  var _tickConsecutiveErrors = 0;
  const _TICK_MAX_ERRORS = 10;

  function tick() {
    try {
      // Pausa se a aba está oculta — cancela o loop completamente
      // (o handler de visibilitychange reinicia quando voltar a ficar visível)
      if (_pageHidden) {
        if (state.progressAnimFrame) {
          cancelAnimationFrame(state.progressAnimFrame);
          state.progressAnimFrame = null;
        }
        return;
      }

      if (!state.isPlaying || state.durationMs <= 0) {
        state.progressAnimFrame = null;
        return;
      }

      // Throttle de frames: a barra anda ~1px/s (com snap de pixel), então
      // 60fps é desperdício — metade dos frames é pulada (~30fps efetivo),
      // cortando pela metade o wakeup do RAF e o trabalho de paint. Durante
      // um resync (glide de reajuste) mantém 60fps para a transição suave.
      _tickFrameCount++;
      if (!state.resyncActive && (_tickFrameCount % 2 === 0)) {
        state.progressAnimFrame = requestAnimationFrame(tick);
        return;
      }

      // Interpola: quanto tempo passou desde a última atualização WebSocket
      const elapsed = performance.now() - state.lastProgressTime;
      const targetNow = Math.min(state.progressMs + elapsed, state.durationMs);

      // Reajuste suave: quando o backend diverge (resyncActive), desliza até o
      // alvo com easeOutCubic ao longo de RESYNC_DURATION_MS — barra, texto e
      // anel juntos. Baseado em TEMPO: sempre termina (t=1 → iguala o alvo),
      // sem lag residual de equilíbrio em nenhuma taxa de frames.
      let displayedMs = targetNow;
      let _settledThisFrame = false;
      if (state.resyncActive) {
        const t = Math.min(1, (performance.now() - state.resyncStartAt) / RESYNC_DURATION_MS);
        const eased = 1 - Math.pow(1 - t, 3);
        displayedMs = state.resyncFrom + (targetNow - state.resyncFrom) * eased;
        if (t >= 1) {
          displayedMs = targetNow;
          state.resyncActive = false;
          _settledThisFrame = true;
          updateProgressSyncStatus();
        }
      }
      state.smoothMs = displayedMs;
      const pct = Math.min(100, (displayedMs / state.durationMs) * 100);

      // Barra + thumb: custom property (transform GPU, sem layout). Atualiza
      // a cada frame do loop já throttled (~30fps) — barato e sem tremor.
      setProgressPct(pct);

      // Texto e anel: a cada frame (30fps efetivos — o texto muda 1x/s e o
      // anel é quantizado, então nunca é perceptível)
      if (els.timeCurrent) {
        const t = formatTime(displayedMs);
        if (t !== _lastTimeText) {
          _lastTimeText = t;
          els.timeCurrent.textContent = t;
        }
      }
      setArtRingProgress(pct);

      // ── Unsynced lyrics (Genius) — calcula linha ativa pela posição interpolada ──
      // Letras sincronizadas são gerenciadas exclusivamente pelo SSE `lyric-line`
      // para evitar race condition com o LyricScheduler do backend.
      if (state.lyrics !== _lyrics) {
        _lyrics = state.lyrics;
        _synced = state.lyricsSynced;
      }
      if (!_synced && Array.isArray(_lyrics) && _lyrics.length > 0 && state.durationMs > 0) {
        const pct = Math.min(1, Math.max(0, displayedMs / state.durationMs));
        const expectedIdx = Math.min(_lyrics.length - 1, Math.floor(pct * _lyrics.length));

        if (expectedIdx !== state.currentLyricIndex) {
          state.currentLyricIndex = expectedIdx;
          // Durante o reajuste suave, linhas intermediárias mudam sem scroll;
          // o render final (resync concluído) rola normalmente.
          renderLyrics(_lyrics, expectedIdx, false, state.resyncActive);
        } else if (_settledThisFrame) {
          // Reajuste terminou e o índice já era o correto: garante o scroll
          scrollLyricsToActive();
        }
      }

      // Frame concluído SEM erro: zera o contador de erros consecutivos.
      // (Colocado no FINAL do try: se renderLyrics etc. lançarem acima, o
      // contador acumula corretamente até o watchdog assumir.)
      _tickConsecutiveErrors = 0;
    } catch (err) {
      // Uma exceção NUNCA pode matar o loop RAF para sempre: erros transientes
      // auto-recuperam no frame seguinte. Se o erro for persistente (≥N erros
      // consecutivos), encerra o loop e o watchdog (1s) tenta reiniciar — evita
      // CPU burn com exceção contínua enquanto mantém a barra/anel vivos.
      _tickConsecutiveErrors++;
      // Loga apenas no primeiro erro de cada burst (o watchdog reinicia a cada 1s)
      if (_tickConsecutiveErrors === 1) {
        DBG.error('PROGRESS', 'tick error — loop mantido vivo (auto-recupera; watchdog reinicia se persistir)', err);
      }
      if (_tickConsecutiveErrors >= _TICK_MAX_ERRORS) {
        state.progressAnimFrame = null;
        return;
      }
      state.progressAnimFrame = requestAnimationFrame(tick);
      return;
    }

    state.progressAnimFrame = requestAnimationFrame(tick);
  }

  state.progressAnimFrame = requestAnimationFrame(tick);
}

function stopProgressAnimation() {
  if (state.progressAnimFrame) {
    cancelAnimationFrame(state.progressAnimFrame);
    state.progressAnimFrame = null;
  }
}

// ── Watchdog de progresso ───────────────────────────────────────────────────
// Garante que a barra e o anel nunca fiquem parados por muito tempo:
// se o loop RAF morrer por qualquer motivo (exceção fora do try/catch,
// race com pause/resume), este timer reinicia em até 1s enquanto toca.
setInterval(function() {
  if (!document.hidden && state.isPlaying && state.durationMs > 0 && !state.progressAnimFrame) {
    startProgressAnimation();
  }
}, 1000);

// ── Page Visibility — retoma RAF quando aba volta a ficar visível ──────────
document.addEventListener('visibilitychange', function() {
  _pageHidden = document.hidden;
  // Classe no body: o CSS pausa orbs e outras animações caras em GPU
  // quando a aba está oculta (economia real de recursos em segundo plano).
  document.body.classList.toggle('page-hidden', document.hidden);
  // Pausa/retoma partículas quando a aba fica oculta/visível
  // (eco mode também pausa — o CSS esconde, mas o spawn continua caro)
  if (particleSystem.running) {
    if (document.hidden || document.body.classList.contains('eco-mode')) {
      particleSystem.pause();
    } else {
      particleSystem.resume();
    }
  }
  // Reinicia animação da barra de progresso se voltou a ficar visível e está tocando
  if (!document.hidden && state.isPlaying && state.durationMs > 0) {
    // Atualiza o time base para compensar o tempo em que ficou oculto
    state.lastProgressTime = performance.now();
    startProgressAnimation();
  }
});

// ── Floating Particles (Spotify-style) ──────────────────────────────────────
/**
 * Sistema de partículas flutuantes que sobem do fundo da tela,
 * similar ao efeito de fundo do Spotify.
 * As cores acompanham o tema ativo.
 */
const particleSystem = (function() {
  // ── Config ──────────────────────────────────────────────────────────
  // Adaptive particle count baseada em performance
  var MAX_PARTICLES = 10;
  var SPAWN_INTERVAL_MS = 1400;
  var MIN_SIZE = 3;
  var MAX_SIZE = 8;

  /**
   * Ajusta a densidade de partículas (0..30) em tempo real.
   * 0 = desliga o spawn e limpa o campo. Reduzir o valor libera nós do DOM
   * imediatamente (memória) e diminui o trabalho de layout/paint.
   * @param {number} n Quantidade máxima de partículas simultâneas
   * @returns {number} Densidade aplicada (clampada)
   */
  function setDensity(n) {
    var count = Math.max(0, Math.min(30, Math.round(n) || 0));
    MAX_PARTICLES = count;
    // IMPORTANTE: nunca 0 — setInterval(fn, 0) vira busy-loop de CPU.
    // Com count=0 o spawn() já early-return (field.children.length >= 0),
    // então o timer nunca cria nós; um intervalo grande é só seguro.
    SPAWN_INTERVAL_MS = count === 0 ? 60000 : Math.max(500, Math.round(14000 / count));

    // Reinicia o timer de spawn se estiver rodando (nova frequência)
    if (running && !paused) {
      if (spawnTimer) {
        clearInterval(spawnTimer);
        spawnTimer = null;
      }
      if (count > 0) {
        spawnTimer = setInterval(function() {
          if (!paused) spawn();
        }, SPAWN_INTERVAL_MS);
      }
    }

    // Limpa excedentes imediatamente (não espera o timer remover)
    if (field && field.children.length > count) {
      while (field.children.length > count) {
        field.removeChild(field.firstChild);
      }
    }
    return count;
  }
  var MIN_DURATION = 15;
  var MAX_DURATION = 32;

  // ── State ───────────────────────────────────────────────────────────
  var field = null;
  var spawnTimer = null;
  var running = false;
  var paused = false;
  var currentColor = 'rgba(139, 92, 246, 0.4)';
  var currentGlow = 'rgba(139, 92, 246, 0.15)';
  /** Cores dinâmicas do tema Auto (null = usa o mapa THEME_COLORS) */
  var autoRgb = null;

  // ── Theme color map ────────────────────────────────────────────────
  var THEME_COLORS = {
    purple: { color: 'rgba(139, 92, 246, 0.4)', glow: 'rgba(139, 92, 246, 0.15)' },
    green:  { color: 'rgba(29, 185, 84, 0.35)',  glow: 'rgba(29, 185, 84, 0.12)' },
    blue:   { color: 'rgba(88, 101, 242, 0.35)', glow: 'rgba(88, 101, 242, 0.12)' },
    orange: { color: 'rgba(249, 115, 22, 0.35)', glow: 'rgba(249, 115, 22, 0.12)' },
    pink:   { color: 'rgba(233, 30, 138, 0.35)',  glow: 'rgba(233, 30, 138, 0.12)' },
    teal:   { color: 'rgba(13, 148, 136, 0.35)',  glow: 'rgba(13, 148, 136, 0.12)' },
    amber:  { color: 'rgba(217, 119, 6, 0.35)',  glow: 'rgba(217, 119, 6, 0.12)' },
    rose:   { color: 'rgba(225, 29, 72, 0.35)',  glow: 'rgba(225, 29, 72, 0.12)' },
    indigo: { color: 'rgba(79, 70, 229, 0.35)',  glow: 'rgba(79, 70, 229, 0.12)' },
  };

  // ── Init ────────────────────────────────────────────────────────────
  function init() {
    field = document.getElementById('particle-field');
    if (!field) {
      // Cria o container se não existir no HTML
      field = document.createElement('div');
      field.id = 'particle-field';
      field.className = 'particle-field';
      field.setAttribute('aria-hidden', 'true');
      document.body.appendChild(field);
    }
  }

  // ── Spawn a single particle ────────────────────────────────────────
  function spawn() {
    if (!field || paused) return;

    // Verifica se já atingiu o máximo
    if (field.children.length >= MAX_PARTICLES) return;

    var size = MIN_SIZE + Math.random() * (MAX_SIZE - MIN_SIZE);
    var duration = MIN_DURATION + Math.random() * (MAX_DURATION - MIN_DURATION);
    var startX = Math.random() * 100; // % da largura
    var drift = (Math.random() - 0.5) * 120; // px de desvio lateral
    var opacity = 0.15 + Math.random() * 0.35;

    var particle = document.createElement('div');
    particle.className = 'particle';
    particle.style.cssText = [
      'left: ' + startX + '%',
      'width: ' + size + 'px',
      'height: ' + size + 'px',
      'background: ' + currentColor,
      'box-shadow: 0 0 ' + (size * 2) + 'px ' + currentGlow,
      '--p-drift: ' + drift + 'px',
      '--p-opacity: ' + opacity,
      'animation-duration: ' + duration + 's',
    ].join('; ');

    field.appendChild(particle);

    // Remove após a animação terminar
    setTimeout(function() {
      if (particle.parentNode) {
        particle.parentNode.removeChild(particle);
      }
    }, duration * 1000 + 100);
  }

  // ── Start spawning ─────────────────────────────────────────────────
  function start() {
    if (running) return;
    running = true;
    paused = false;

    // Densidade 0 (desligado) ou aba oculta: não spawna nada — sem timer.
    if (MAX_PARTICLES <= 0 || document.hidden) return;

    // Spawn imediato de algumas partículas para não começar vazio
    // Burst inicial menor — 5 partículas em vez de 8
    for (var i = 0; i < 5; i++) {
      setTimeout(function() { spawn(); }, i * 250);
    }

    spawnTimer = setInterval(function() {
      if (!paused) spawn();
    }, SPAWN_INTERVAL_MS);
  }

  // ── Stop spawning e limpa ─────────────────────────────────────────
  function stop() {
    running = false;
    paused = false;
    if (spawnTimer) {
      clearInterval(spawnTimer);
      spawnTimer = null;
    }
    if (field) {
      while (field.firstChild) {
        field.removeChild(field.firstChild);
      }
    }
  }

  // ── Pause (sem limpar) ────────────────────────────────────────────
  function pause() {
    paused = true;
  }

  // ── Resume ────────────────────────────────────────────────────────
  function resume() {
    paused = false;
  }

  // ── Update colors based on theme ──────────────────────────────────
  function updateColors(themeId) {
    var colors;
    if (themeId === 'auto' && autoRgb) {
      colors = {
        color: 'rgba(' + autoRgb.r + ', ' + autoRgb.g + ', ' + autoRgb.b + ', 0.4)',
        glow: 'rgba(' + autoRgb.r + ', ' + autoRgb.g + ', ' + autoRgb.b + ', 0.15)',
      };
    } else {
      colors = THEME_COLORS[themeId] || THEME_COLORS.purple;
    }
    currentColor = colors.color;
    currentGlow = colors.glow;

    // Atualiza partículas existentes com transição CSS
    if (field) {
      var particles = field.children;
      for (var i = 0; i < particles.length; i++) {
        var p = particles[i];
        p.style.background = currentColor;
        p.style.boxShadow = '0 0 ' + (parseInt(p.style.width) * 2 || 10) + 'px ' + currentGlow;
      }
    }
  }

  /** Define as cores dinâmicas do tema Auto (null restaura o mapa padrão). */
  function setAutoColors(rgb) {
    autoRgb = rgb || null;
  }

  // ── Public API ────────────────────────────────────────────────────
  return {
    init: init,
    start: start,
    stop: stop,
    pause: pause,
    resume: resume,
    updateColors: updateColors,
    setAutoColors: setAutoColors,
    setDensity: setDensity,
    get running() { return running; },
  };
})();

// ── Theme Selector ───────────────────────────────────────────────────────────
/** Temas oficiais — FONTE ÚNICA (alinhada com o CSS e o HTML) */
const THEMES = ['auto', 'purple', 'green', 'blue', 'orange', 'pink', 'teal', 'amber', 'rose', 'indigo'];
let themeDropdownOpen = false;

// ── Auto Theme (cores da capa do álbum) ────────────────────────────────────────
/** Cache de paletas extraídas por URL (limite evita vazamento de memória) */
const AUTO_THEME_CACHE = new Map();
const AUTO_CACHE_MAX = 24;

// ── Fila de transição (cliques rápidos não acumulam overlays) ───────────────
let _themeQueue = [];
let _themeBusy = false;

/**
 * Aplica um tema. Com skipTransition=false faz um fade suave usando UM único
 * overlay estático (#theme-overlay, já presente no HTML) — sem criar overlays
 * dinâmicos nem duplicar animações. Cliques rápidos são enfileirados.
 */
function applyTheme(themeId, skipTransition) {
  // Valida contra a fonte única (fallback: purple)
  themeId = THEMES.includes(themeId) ? themeId : 'purple';

  try { localStorage.setItem('discord-lyrics-theme', themeId); } catch {}

  if (skipTransition) {
    setThemeDirect(themeId);
    return;
  }

  _themeQueue.push(themeId);
  if (!_themeBusy) _processThemeQueue();
}

function _processThemeQueue() {
  if (_themeQueue.length === 0) { _themeBusy = false; return; }
  _themeBusy = true;

  const overlay = document.getElementById('theme-overlay');
  const themeId = _themeQueue.shift();

  if (!overlay) {
    // Fallback sem overlay: aplica direto
    setThemeDirect(themeId);
    _themeBusy = false;
    if (_themeQueue.length) _processThemeQueue();
    return;
  }

  // Fade IN (cobre a cena)
  overlay.classList.add('active');

  setTimeout(function() {
    // Troca o tema enquanto o overlay está ativo (cores mudam invisivelmente)
    setThemeDirect(themeId);

    // Fade OUT — revela o novo tema
    overlay.classList.remove('active');

    // Processa o próximo após o fade completar
    setTimeout(function() {
      _themeBusy = false;
      if (_themeQueue.length) _processThemeQueue();
    }, 180);
  }, 180);
}

/**
 * Aplica o tema diretamente no DOM sem animação.
 */
function setThemeDirect(themeId) {
  document.documentElement.setAttribute('data-theme', themeId);

  // Atualiza o botão do seletor
  const btn = document.getElementById('theme-btn');
  if (btn) btn.setAttribute('data-current', themeId);

  // Marca a opção ativa no dropdown
  document.querySelectorAll('.theme-option').forEach(function(opt) {
    opt.classList.toggle('active', opt.getAttribute('data-theme-id') === themeId);
  });

  // Marca o card ativo no grid de configurações (se já carregado)
  if (typeof settingsThemeGrid !== 'undefined' && settingsThemeGrid) {
    settingsThemeGrid.querySelectorAll('.theme-card').forEach(function(card) {
      card.classList.toggle('active', card.getAttribute('data-theme-id') === themeId);
    });
  }

  // Atualiza a cor das partículas flutuantes
  if (particleSystem && particleSystem.updateColors) {
    particleSystem.updateColors(themeId);
  }

  // Tema Auto: extrai cores da capa atual (assíncrono, com fallback)
  if (themeId === 'auto') {
    refreshAutoTheme(state.albumArtUrl);
  } else {
    // Saiu do Auto: limpa as vars inline para o tema selecionado valer
    // (as partículas já foram atualizadas acima com updateColors(themeId))
    _clearAutoCssVars();
    if (_autoTweenRaf) { cancelAnimationFrame(_autoTweenRaf); _autoTweenRaf = null; }
    _autoPrevDerived = null;
  }
}

/**
 * Carrega o tema salvo do localStorage (fallback: purple).
 * Sem transição na carga inicial. As partículas já são iniciadas
 * no DOMContentLoaded — aqui só aplicamos o tema.
 */
function loadSavedTheme() {
  let saved = null;
  try { saved = localStorage.getItem('discord-lyrics-theme'); } catch {}
  applyTheme(THEMES.includes(saved) ? saved : 'green', true);
}

// ── Helpers de cor (RGB/HSL) ───────────────────────────────────────────────────
function _clamp255(v) { return Math.max(0, Math.min(255, Math.round(v))); }

function _rgbToHsl(r, g, b) {
  r /= 255; g /= 255; b /= 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b);
  let h = 0, s = 0, l = (max + min) / 2;
  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    switch (max) {
      case r: h = (g - b) / d + (g < b ? 6 : 0); break;
      case g: h = (b - r) / d + 2; break;
      default: h = (r - g) / d + 4; break;
    }
    h /= 6;
  }
  return { h: h, s: s, l: l };
}

function _hslToRgb(h, s, l) {
  let r, g, b;
  if (s === 0) {
    r = g = b = l;
  } else {
    const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
    const p = 2 * l - q;
    const hue2rgb = (p2, q2, t) => {
      if (t < 0) t += 1;
      if (t > 1) t -= 1;
      if (t < 1 / 6) return p2 + (q2 - p2) * 6 * t;
      if (t < 1 / 2) return q2;
      if (t < 2 / 3) return p2 + (q2 - p2) * (2 / 3 - t) * 6;
      return p2;
    };
    r = hue2rgb(p, q, h + 1 / 3);
    g = hue2rgb(p, q, h);
    b = hue2rgb(p, q, h - 1 / 3);
  }
  return { r: _clamp255(r * 255), g: _clamp255(g * 255), b: _clamp255(b * 255) };
}

function _rgbToHex(rgb) {
  return '#' + [rgb.r, rgb.g, rgb.b].map((v) => v.toString(16).padStart(2, '0')).join('');
}

/** Torna uma cor vibrante e legível sobre o fundo escuro do app. */
function _accentFrom(rgb, minLightness, minSaturation) {
  const hsl = _rgbToHsl(rgb.r, rgb.g, rgb.b);
  return _hslToRgb(hsl.h, Math.max(minSaturation, hsl.s), Math.max(minLightness, hsl.l));
}

/**
 * Extrai uma PALETA rica da capa via canvas (amostra 32x32).
 * Quantiza em buckets (5 bits/canal), agrupa por proximidade e seleciona
 * até 4 cores DISTINTAS entre si — dá ao tema Auto "mais cores" de verdade.
 * Retorna { palette: [{r,g,b}…4], average, vibrant } ou null em falha/CORS.
 * Resultado é cacheado por URL.
 */
function extractAlbumColors(url, cb) {
  if (!url || typeof url !== 'string') { cb(null); return; }
  if (AUTO_THEME_CACHE.has(url)) { cb(AUTO_THEME_CACHE.get(url)); return; }

  const img = new Image();
  img.crossOrigin = 'anonymous';

  img.onload = function() {
    try {
      const size = 32;
      const canvas = document.createElement('canvas');
      canvas.width = size;
      canvas.height = size;
      const ctx = canvas.getContext('2d');
      ctx.drawImage(img, 0, 0, size, size);
      const data = ctx.getImageData(0, 0, size, size).data;

      // Buckets quantizados (32 níveis por canal) para agrupar cores iguais
      const bucketMap = new Map();
      let rSum = 0, gSum = 0, bSum = 0, n = 0;
      let vr = 0, vg = 0, vb = 0, bestSat = -1;

      for (let i = 0; i < data.length; i += 4) {
        const R = data[i], G = data[i + 1], B = data[i + 2], A = data[i + 3];
        if (A < 128) continue;
        rSum += R; gSum += G; bSum += B; n++;
        const max = Math.max(R, G, B), min = Math.min(R, G, B);
        const sat = max === 0 ? 0 : (max - min) / max;
        if (sat > bestSat) { bestSat = sat; vr = R; vg = G; vb = B; }

        const key = (R >> 3) * 1024 + (G >> 3) * 32 + (B >> 3);
        let bkt = bucketMap.get(key);
        if (!bkt) { bkt = { r: 0, g: 0, b: 0, c: 0 }; bucketMap.set(key, bkt); }
        bkt.r += R; bkt.g += G; bkt.b += B; bkt.c++;
      }
      if (n === 0) { cb(null); return; }

      const average = { r: rSum / n, g: gSum / n, b: bSum / n };
      // Vibrante = blend 50/50 entre média e o pixel mais saturado
      const vibrant = {
        r: (average.r + vr) / 2,
        g: (average.g + vg) / 2,
        b: (average.b + vb) / 2,
      };

      // Buckets mais populares (média por bucket), ordenados por contagem
      const buckets = Array.from(bucketMap.values())
        .map(function(b) { return { r: b.r / b.c, g: b.g / b.c, b: b.b / b.c, c: b.c }; })
        .sort(function(a, b2) { return b2.c - a.c; });

      // Seleciona até 4 cores com matiz bem separado (nada de clones)
      const palette = [];
      for (let i = 0; i < buckets.length && palette.length < 4; i++) {
        const col = buckets[i];
        const hsl = _rgbToHsl(col.r, col.g, col.b);
        // Quase-pretos/brancos não servem de acento (só de fundo)
        if (hsl.l < 0.10 || hsl.l > 0.90) continue;
        let distinct = true;
        for (let j = 0; j < palette.length; j++) {
          const p = _rgbToHsl(palette[j].r, palette[j].g, palette[j].b);
          let dh = Math.abs(hsl.h - p.h);
          if (dh > 0.5) dh = 1 - dh;
          if (dh < 0.07) { distinct = false; break; }
        }
        if (distinct) palette.push({ r: col.r, g: col.g, b: col.b });
      }
      if (palette.length === 0) {
        palette.push({ r: vibrant.r, g: vibrant.g, b: vibrant.b });
      }

      const colors = { palette: palette, average: average, vibrant: vibrant };
      if (AUTO_THEME_CACHE.size >= AUTO_CACHE_MAX) {
        AUTO_THEME_CACHE.delete(AUTO_THEME_CACHE.keys().next().value);
      }
      AUTO_THEME_CACHE.set(url, colors);
      cb(colors);
    } catch {
      // Canvas sujo por CORS ou erro de decoding — usa fallback
      cb(null);
    }
  };

  img.onerror = function() { cb(null); };
  img.src = url;
}

/** Última paleta derivada aplicada (ponto de partida do próximo fade) */
let _autoPrevDerived = null;
/** RAF ativo do tween de cores (cancelado em trocas rápidas) */
let _autoTweenRaf = null;
/** Duração do fade entre paletas ao trocar de faixa */
const AUTO_TWEEN_MS = 650;

/** Interpola linearmente dois RGB e arredonda para ints (0-255). */
function _lerpRgb(a, b, t) {
  return {
    r: Math.round(a.r + (b.r - a.r) * t),
    g: Math.round(a.g + (b.g - a.g) * t),
    b: Math.round(a.b + (b.b - a.b) * t),
  };
}

/** True se duas paletas derivadas têm acentos idênticos (evita RAF desnecessário). */
function _sameDerived(a, b) {
  return a && b
    && a.primary.r === b.primary.r && a.primary.g === b.primary.g && a.primary.b === b.primary.b
    && a.secondary.r === b.secondary.r && a.secondary.g === b.secondary.g && a.secondary.b === b.secondary.b
    && a.tertiary.r === b.tertiary.r && a.tertiary.g === b.tertiary.g && a.tertiary.b === b.tertiary.b
    && a.quaternary.r === b.quaternary.r && a.quaternary.g === b.quaternary.g && a.quaternary.b === b.quaternary.b;
}

/**
 * Deriva o conjunto completo de cores do tema Auto a partir da paleta
 * extraída da capa (acentos + orbs + fundos derivados do matiz).
 */
function _autoDerive(colors) {
  const pal = colors.palette || [];
  const c0 = pal[0] || colors.vibrant || colors.average;
  const c1 = pal[1] || colors.average || c0;
  const c2 = pal[2] || c1;
  const c3 = pal[3] || c2;

  const primary = _accentFrom(c0, 0.55, 0.45);
  const secondary = _accentFrom(c1, 0.7, 0.35);
  const tertiary = _accentFrom(c2, 0.62, 0.38);
  const quaternary = _accentFrom(c3, 0.55, 0.35);
  const hue = _rgbToHsl(primary.r, primary.g, primary.b).h;

  return {
    primary: primary,
    secondary: secondary,
    tertiary: tertiary,
    quaternary: quaternary,
    hue: hue,
    bg1: _hslToRgb(hue, 0.28, 0.06),
    bg2: _hslToRgb(hue, 0.24, 0.1),
    card: _hslToRgb(hue, 0.22, 0.09),
    elev: _hslToRgb(hue, 0.2, 0.14),
  };
}

/** Aplica um conjunto derivado nas variáveis CSS do tema auto. */
function _autoApply(d) {
  const root = document.documentElement;

  root.style.setProperty('--accent-primary', _rgbToHex(d.primary));
  root.style.setProperty('--accent-primary-rgb', d.primary.r + ', ' + d.primary.g + ', ' + d.primary.b);
  root.style.setProperty('--accent-primary-glow', 'rgba(' + d.primary.r + ', ' + d.primary.g + ', ' + d.primary.b + ', 0.35)');
  root.style.setProperty('--accent-secondary', _rgbToHex(d.secondary));
  root.style.setProperty('--accent-secondary-rgb', d.secondary.r + ', ' + d.secondary.g + ', ' + d.secondary.b);
  // Tint do overlay do modo ambiente — cor dominante da paleta da capa.
  // O .ambient-bg::after usa var(--ambient-tint-rgb) para um glow coerente.
  root.style.setProperty('--ambient-tint-rgb', d.primary.r + ', ' + d.primary.g + ', ' + d.primary.b);
  // Gradiente com 3 cores reais da capa (não mais 2 derivadas de um matiz)
  root.style.setProperty('--accent-gradient-start', _rgbToHex(d.primary));
  root.style.setProperty('--accent-gradient-mid', _rgbToHex(d.tertiary));
  root.style.setProperty('--accent-gradient-end', _rgbToHex(d.secondary));

  // Orbs com 3 cores distintas da paleta (fundo mais colorido)
  root.style.setProperty('--orb-1-color',
    'rgba(' + d.primary.r + ', ' + d.primary.g + ', ' + d.primary.b + ', 0.15), rgba(' + d.tertiary.r + ', ' + d.tertiary.g + ', ' + d.tertiary.b + ', 0.06)');
  root.style.setProperty('--orb-2-color',
    'rgba(' + d.secondary.r + ', ' + d.secondary.g + ', ' + d.secondary.b + ', 0.10), rgba(' + d.quaternary.r + ', ' + d.quaternary.g + ', ' + d.quaternary.b + ', 0.04)');
  root.style.setProperty('--orb-3-color',
    'rgba(' + d.tertiary.r + ', ' + d.tertiary.g + ', ' + d.tertiary.b + ', 0.08)');

  // Fundos escuros derivados do matiz dominante da capa
  root.style.setProperty('--bg-primary', _rgbToHex(d.bg1));
  root.style.setProperty('--bg-secondary', _rgbToHex(d.bg2));
  root.style.setProperty('--bg-card', 'rgba(' + d.card.r + ', ' + d.card.g + ', ' + d.card.b + ', 0.88)');
  root.style.setProperty('--bg-elevated', 'rgba(' + d.elev.r + ', ' + d.elev.g + ', ' + d.elev.b + ', 0.92)');
  root.style.setProperty('--bg-gradient',
    'radial-gradient(ellipse at 20% 50%, rgba(' + d.primary.r + ', ' + d.primary.g + ', ' + d.primary.b + ', 0.05), transparent 60%), '
    + 'radial-gradient(ellipse at 50% 100%, rgba(' + d.tertiary.r + ', ' + d.tertiary.g + ', ' + d.tertiary.b + ', 0.04), transparent 60%), '
    + 'radial-gradient(ellipse at 80% 50%, rgba(' + d.secondary.r + ', ' + d.secondary.g + ', ' + d.secondary.b + ', 0.04), transparent 60%)');
}

/** Sincroniza as partículas com a cor primária do tema Auto. */
function _applyAutoParticles(primary) {
  if (particleSystem && particleSystem.setAutoColors) {
    particleSystem.setAutoColors(primary);
    particleSystem.updateColors('auto');
  }
}

/**
 * Aplica a paleta extraída como variáveis CSS do tema auto com FADE suave:
 * ao trocar de faixa, as cores deslizam da paleta anterior para a nova
 * (RAF + easeInOutCubic). Aplicação instantânea na primeira vez, em
 * prefers-reduced-motion e no modo econômico (menos animação).
 */
function applyAutoColors(colors) {
  if (!colors) { resetAutoColors(); return; }

  // Cancela QUALQUER tween em andamento logo no início (troca rápida de faixa,
  // incluindo os caminhos instantâneos/iguais abaixo) — nada fica pendente.
  if (_autoTweenRaf) { cancelAnimationFrame(_autoTweenRaf); _autoTweenRaf = null; }

  const next = _autoDerive(colors);
  const reduceMotion = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const ecoMode = document.body && document.body.classList.contains('eco-mode');

  // Paleta idêntica à atual (mesma capa re-extraída) → sem RAF, sem fade
  if (_sameDerived(_autoPrevDerived, next)) {
    _autoApply(next);
    _autoPrevDerived = next;
    return;
  }

  // Sem paleta anterior, reduced-motion ou eco-mode → aplica direto (sem fade)
  if (!_autoPrevDerived || reduceMotion || ecoMode) {
    _autoApply(next);
    _autoPrevDerived = next;
    _applyAutoParticles(next.primary);
    return;
  }

  // (O RAF já foi cancelado no topo da função — aqui recomeça do ponto atual)
  const from = _autoPrevDerived;
  const start = performance.now();

  const step = function(now) {
    const t = Math.min(1, (now - start) / AUTO_TWEEN_MS);
    // easeInOutCubic
    const e = t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;

    const primary = _lerpRgb(from.primary, next.primary, e);
    const secondary = _lerpRgb(from.secondary, next.secondary, e);
    const tertiary = _lerpRgb(from.tertiary, next.tertiary, e);
    const quaternary = _lerpRgb(from.quaternary, next.quaternary, e);
    const hue = _rgbToHsl(primary.r, primary.g, primary.b).h;

    // Mantém o estado SEMPRE no frame interpolado atual: se uma troca rápida
    // cancelar este tween, o próximo parte exatamente de onde os olhos estão
    // (sem "pulo" para trás ao valor original).
    _autoPrevDerived = {
      primary: primary,
      secondary: secondary,
      tertiary: tertiary,
      quaternary: quaternary,
      hue: hue,
      bg1: _hslToRgb(hue, 0.28, 0.06),
      bg2: _hslToRgb(hue, 0.24, 0.1),
      card: _hslToRgb(hue, 0.22, 0.09),
      elev: _hslToRgb(hue, 0.2, 0.14),
    };

    _autoApply(_autoPrevDerived);

    if (t < 1) {
      _autoTweenRaf = requestAnimationFrame(step);
    } else {
      _autoTweenRaf = null;
      _applyAutoParticles(next.primary);
    }
  };

  _autoTweenRaf = requestAnimationFrame(step);
}

/** Remove apenas as variáveis CSS dinâmicas do tema Auto (sem tocar partículas). */
function _clearAutoCssVars() {
  const root = document.documentElement;
  [
    '--accent-primary', '--accent-primary-rgb', '--accent-primary-glow',
    '--accent-secondary', '--accent-secondary-rgb',
    '--ambient-tint-rgb',
    '--accent-gradient-start', '--accent-gradient-mid', '--accent-gradient-end',
    '--orb-1-color', '--orb-2-color', '--orb-3-color',
    '--bg-primary', '--bg-secondary', '--bg-card', '--bg-elevated', '--bg-gradient',
  ].forEach((v) => root.style.removeProperty(v));
}

/** Remove as cores dinâmicas (volta aos fallbacks do [data-theme="auto"]). */
function resetAutoColors() {
  if (_autoTweenRaf) { cancelAnimationFrame(_autoTweenRaf); _autoTweenRaf = null; }
  _autoPrevDerived = null;
  _clearAutoCssVars();
  if (particleSystem && particleSystem.setAutoColors) {
    particleSystem.setAutoColors(null);
    particleSystem.updateColors('auto');
  }
}

/**
 * Se o tema ativo é 'auto', extrai cores da capa atual (ou limpa sem arte).
 * Chamado sempre que a capa muda e na ativação do tema.
 */
/** URL da última solicitação — evita aplicar extração stale (race async) */
let _autoThemeRequestUrl = null;

function refreshAutoTheme(url) {
  if (document.documentElement.getAttribute('data-theme') !== 'auto') return;
  _autoThemeRequestUrl = url || null;
  if (!url || url.indexOf('data:') === 0) { resetAutoColors(); return; }
  extractAlbumColors(url, function(colors) {
    // Guard: usuário pode ter trocado de tema OU de faixa durante a extração
    if (document.documentElement.getAttribute('data-theme') !== 'auto') return;
    if (_autoThemeRequestUrl !== url) return; // chegou resposta stale
    if (colors) applyAutoColors(colors); else resetAutoColors();
  });
}

// Theme dropdown toggle
var themeBtn = document.getElementById('theme-btn');
var themeDropdown = document.getElementById('theme-dropdown');

if (themeBtn && themeDropdown) {
  // Click no botão abre/fecha
  themeBtn.addEventListener('click', function(e) {
    e.stopPropagation();
    themeDropdownOpen = !themeDropdownOpen;
    themeDropdown.classList.toggle('open', themeDropdownOpen);
  });

  // Click nas opções
  themeDropdown.addEventListener('click', function(e) {
    var opt = e.target.closest('.theme-option');
    if (opt) {
      var themeId = opt.getAttribute('data-theme-id');
      if (themeId) {
        applyTheme(themeId);
        themeDropdownOpen = false;
        themeDropdown.classList.remove('open');
      }
    }
  });

  // Fecha dropdown ao clicar fora
  document.addEventListener('click', function() {
    if (themeDropdownOpen) {
      themeDropdownOpen = false;
      themeDropdown.classList.remove('open');
    }
  });

  // Fecha dropdown com Escape
  document.addEventListener('keydown', function(e) {
    if (e.key === 'Escape' && themeDropdownOpen) {
      themeDropdownOpen = false;
      themeDropdown.classList.remove('open');
    }
  });
}

// ── Auto Cycle Themes ────────────────────────────────────────────────────────
// Intervalo maior: menos churn de transições/overlays por minuto (menos CPU)
const CYCLE_INTERVAL_MS = 6000;
let autoCycleTimer = null;
let autoCycling = false;

function toggleAutoCycle() {
  const btn = document.getElementById('auto-cycle-btn');

  if (autoCycling) {
    clearInterval(autoCycleTimer);
    autoCycleTimer = null;
    autoCycling = false;
    if (btn) btn.classList.remove('active');
    return;
  }

  autoCycling = true;
  if (btn) btn.classList.add('active');

  // Avança imediatamente para o próximo tema
  nextThemeInCycle();

  autoCycleTimer = setInterval(function() {
    nextThemeInCycle();
  }, CYCLE_INTERVAL_MS);
}

// Temas do ciclo automático — exclui o 'auto' (dinâmico, extrairia cores
// a cada ciclo sem necessidade)
const CYCLE_THEMES = THEMES.filter(function(t) { return t !== 'auto'; });

function nextThemeInCycle() {
  var current = document.documentElement.getAttribute('data-theme') || 'purple';
  var idx = CYCLE_THEMES.indexOf(current);
  // Se o atual é 'auto' (fora da lista), parte do ÚLTIMO tema para que o
  // próximo ciclo comece naturalmente pelo primeiro (purple)
  if (idx === -1) idx = CYCLE_THEMES.length - 1;
  var nextIdx = (idx + 1) % CYCLE_THEMES.length;
  // setThemeDirect (via applyTheme) já atualiza o grid de configurações
  applyTheme(CYCLE_THEMES[nextIdx]);
}

// Botão de ciclo automático
var autoCycleBtn = document.getElementById('auto-cycle-btn');
if (autoCycleBtn) {
  autoCycleBtn.addEventListener('click', function(e) {
    e.stopPropagation();
    toggleAutoCycle();
  });
}

// ── Settings Drawer ─────────────────────────────────────────────────────────
var settingsDrawer = document.getElementById('settings-drawer');
var settingsOverlay = document.getElementById('settings-overlay');
var settingsBtn = document.getElementById('settings-btn');
var settingsClose = document.getElementById('settings-close');
var settingsThemeGrid = document.getElementById('settings-theme-grid');
var syncLeadSlider = document.getElementById('sync-lead-slider');
var syncLeadValue = document.getElementById('sync-lead-value');

// Botão de engrenagem
if (settingsBtn) {
  settingsBtn.addEventListener('click', function() {
    openSettings();
    // Sincroniza o theme grid com o tema atual
    updateSettingsThemeUI();
    // Sincroniza o slider com o valor salvo
    loadSyncLeadSetting();
    // Sincroniza os readouts ao vivo (lead + latência) antes do próximo evento
    updateSyncMetrics();
    // Sincroniza os emotes personalizados
    loadEmoteSettings();
    // Sincroniza o campo do token do Discord
    loadTokenSettings();
    // Sincroniza os toggles de Desempenho e a memória em uso
    applySavedPerfSettings();
    updateMemoryUsage();
  });
}

// Botão fechar
if (settingsClose) {
  settingsClose.addEventListener('click', closeSettings);
}

// Fecha ao clicar no overlay
if (settingsOverlay) {
  settingsOverlay.addEventListener('click', closeSettings);
}

// Fecha com Escape
if (settingsDrawer) {
  document.addEventListener('keydown', function(e) {
    if (e.key === 'Escape' && settingsDrawer.classList.contains('open')) {
      closeSettings();
    }
  });
}

// Atualiza o grid de temas nas configurações
function updateSettingsThemeUI() {
  if (!settingsThemeGrid) return;
  var current = document.documentElement.getAttribute('data-theme') || 'purple';
  settingsThemeGrid.querySelectorAll('.theme-card').forEach(function(card) {
    card.classList.toggle('active', card.getAttribute('data-theme-id') === current);
  });
}

// Tema no settings grid
if (settingsThemeGrid) {
  settingsThemeGrid.addEventListener('click', function(e) {
    var card = e.target.closest('.theme-card');
    if (card) {
      var themeId = card.getAttribute('data-theme-id');
      if (themeId) {
        // applyTheme (com fila) aplica e o setThemeDirect interno já sincroniza o grid
        applyTheme(themeId);
        addLog('info', 'SETTINGS', 'Theme changed to ' + themeId.charAt(0).toUpperCase() + themeId.slice(1));
      }
    }
  });
}

// Sync lead slider
function loadSyncLeadSetting() {
  var saved = null;
  try { saved = localStorage.getItem('discord-lyrics-sync-lead'); } catch {}
  var val = saved !== null ? parseInt(saved, 10) : 45;
  if (syncLeadSlider) syncLeadSlider.value = val;
  if (syncLeadValue) syncLeadValue.textContent = val + 'ms';
}

// Emotes personalizados do status
/** Sincroniza os inputs de emotes com os valores salvos (vindos do backend). */
function loadEmoteSettings() {
  var trackInput = document.getElementById('opt-track-emoji');
  if (trackInput) trackInput.value = state.trackEmoji || '';
  var lyricsInput = document.getElementById('opt-lyrics-emoji');
  if (lyricsInput) lyricsInput.value = state.lyricsEmoji || '';
  // Mostra o emote salvo no span de valor (fallback visual quando vazio)
  var trackVal = document.getElementById('opt-track-emoji-value');
  if (trackVal) trackVal.textContent = state.trackEmoji || '🎵';
  var lyricsVal = document.getElementById('opt-lyrics-emoji-value');
  if (lyricsVal) lyricsVal.textContent = state.lyricsEmoji || '🎤';
}

function loadTokenSettings() {
  var tokenInput = document.getElementById('opt-discord-token');
  if (tokenInput) tokenInput.value = '';
  var statusEl = document.getElementById('discord-token-status');
  if (statusEl) {
    statusEl.textContent = state.tokenConfigured ? 'Configurado' : 'Não configurado';
    statusEl.setAttribute('data-status', state.tokenConfigured ? 'configured' : 'missing');
  }
  var feedbackEl = document.getElementById('discord-token-feedback');
  if (feedbackEl) feedbackEl.textContent = '';
  updateDiscordAccountUI();
}

function updateDiscordAccountUI() {
  var panel = document.getElementById('discord-account-info');
  var avatar = document.getElementById('discord-account-avatar');
  var username = document.getElementById('discord-account-username');
  var tag = document.getElementById('discord-account-tag');
  if (!panel || !avatar || !username || !tag) return;

  if (state.discordAccountName) {
    panel.style.display = 'flex';
    username.textContent = state.discordAccountName;
    tag.textContent = state.discordAccountTag || '';
    if (state.discordAccountAvatarUrl) {
      avatar.style.backgroundImage = 'url(' + state.discordAccountAvatarUrl + ')';
    } else {
      avatar.style.backgroundImage = '';
    }
  } else {
    panel.style.display = 'none';
    avatar.style.backgroundImage = '';
    username.textContent = '';
    tag.textContent = '';
  }
}

function showTokenFeedback(msg, type) {
  var el = document.getElementById('discord-token-feedback');
  if (!el) return;
  el.textContent = msg;
  el.className = 'settings-hint ' + (type || 'info');
}

async function saveDiscordToken() {
  var tokenInput = document.getElementById('opt-discord-token');
  var statusEl = document.getElementById('discord-token-status');
  var saveBtn = document.getElementById('btn-save-discord-token');
  if (!tokenInput || !saveBtn) return;

  var token = tokenInput.value.trim();
  if (!token) {
    showTokenFeedback('Por favor, insira o token.', 'warn');
    return;
  }

  saveBtn.disabled = true;
  saveBtn.textContent = 'Verificando...';
  showTokenFeedback('', 'info');

  try {
    var res = await fetch('/api/setup', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token }),
    });
    var data = await res.json();

    if (data.ok) {
      showTokenFeedback('✅ Token salvo com sucesso!', 'ok');
      if (statusEl) {
        statusEl.textContent = 'Configurado';
        statusEl.setAttribute('data-status', 'configured');
      }
      state.tokenConfigured = true;
      tokenInput.value = '';
      state.discordAccountName = data.username || null;
      state.discordAccountTag = data.tag || null;
      state.discordAccountAvatarUrl = data.avatarUrl || null;
      updateDiscordAccountUI();
      addLog('success', 'SETUP', 'Logged in as ' + (data.username || data.tag || 'user'));
      addLog('success', 'APP', 'Token configured. Starting monitor.');
      try {
        const stateRes = await fetch('/api/status');
        const newState = await stateRes.json();
        updateUI(newState);
      } catch (_err) {
        // Não é crítico se a requisição de status falhar aqui
      }
    } else {
      showTokenFeedback(data.error || 'Erro ao validar token.', 'warn');
    }
  } catch (err) {
    console.error('[Setup] Failed to save token:', err);
    showTokenFeedback('Erro ao conectar com o servidor.', 'warn');
  } finally {
    saveBtn.disabled = false;
    saveBtn.textContent = 'Salvar token';
  }
}

/** Exibe feedback do estado dos emotes na seção de configurações. */
function showEmoteFeedback(msg, type) {
  var el = document.getElementById('emote-feedback');
  if (!el) return;
  el.textContent = msg;
  el.className = 'settings-hint emote-feedback ' + (type || 'info');
}

/**
 * Testa um emote imediatamente no status do Discord SEM persistir no config.
 * @param {'trackEmoji'|'lyricsEmoji'} key - Qual campo está sendo testado
 */
function testEmote(key, value) {
  var val = (value || '').trim();
  var btn = document.getElementById(key === 'lyricsEmoji' ? 'btn-test-lyrics-emoji' : 'btn-test-track-emoji');
  if (btn) {
    btn.disabled = true;
    setTimeout(function() { btn.disabled = false; }, 1500);
  }

  if (!state.tokenConfigured) {
    showEmoteFeedback('⚠️ Token do Discord não configurado — configure antes de testar.', 'warn');
    return;
  }

  var isLyrics = key === 'lyricsEmoji';
  if (!state.trackName) {
    showEmoteFeedback('ℹ️ Nada tocando no momento — inicie uma música e teste de novo.', 'info');
  } else if (isLyrics && state.currentLyricIndex < 0) {
    showEmoteFeedback('ℹ️ Sem linha de letra ativa agora — teste quando uma letra estiver na tela.', 'info');
  } else {
    showEmoteFeedback('✅ Emote enviado! Confira o status do Discord.', 'ok');
  }

  fetch('/api/settings', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ [key]: val, testEmotes: true }),
  }).catch(function(err) {
    console.error('[Settings] Failed to test emote:', err);
    showEmoteFeedback('⚠️ Erro ao testar emote — backend offline?', 'warn');
  });
}

/** Salva (persiste) os dois emotes e os aplica ao status. */
function saveEmoteSettings() {
  var payload = {};
  var trackInput = document.getElementById('opt-track-emoji');
  var lyricsInput = document.getElementById('opt-lyrics-emoji');
  if (trackInput) payload.trackEmoji = trackInput.value.trim();
  if (lyricsInput) payload.lyricsEmoji = lyricsInput.value.trim();

  fetch('/api/settings', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  }).then(function(res) { return res.json(); })
    .then(function(data) {
      if (data && data.ok) {
        showEmoteFeedback('💾 Emotes salvos e aplicados ao status!', 'ok');
      } else {
        showEmoteFeedback('⚠️ Falha ao salvar emotes.', 'warn');
      }
    })
    .catch(function(err) {
      console.error('[Settings] Failed to save emotes:', err);
      showEmoteFeedback('⚠️ Erro ao salvar emotes — backend offline?', 'warn');
    });
}

/** Restaura os valores salvos e reaplica o status com eles (desfaz um teste). */
function revertEmotes() {
  loadEmoteSettings();
  fetch('/api/settings', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ revertEmotes: true }),
  }).catch(function(err) {
    console.error('[Settings] Failed to revert emotes:', err);
  });
  showEmoteFeedback('↩️ Emotes restaurados para os valores salvos.', 'info');
}

/** Liga os inputs e botões de emotes do drawer de configurações. */
function initEmoteSettings() {
  var trackInput = document.getElementById('opt-track-emoji');
  var lyricsInput = document.getElementById('opt-lyrics-emoji');
  var trackVal = document.getElementById('opt-track-emoji-value');
  var lyricsVal = document.getElementById('opt-lyrics-emoji-value');

  // Preview ao vivo: o span de valor acompanha o que está no input
  if (trackInput && trackVal) {
    trackInput.addEventListener('input', function() {
      trackVal.textContent = this.value.trim() || '🎵';
    });
  }
  if (lyricsInput && lyricsVal) {
    lyricsInput.addEventListener('input', function() {
      lyricsVal.textContent = this.value.trim() || '🎤';
    });
  }

  var testTrackBtn = document.getElementById('btn-test-track-emoji');
  if (testTrackBtn && trackInput) {
    testTrackBtn.addEventListener('click', function() {
      testEmote('trackEmoji', trackInput.value);
    });
  }
  var testLyricsBtn = document.getElementById('btn-test-lyrics-emoji');
  if (testLyricsBtn && lyricsInput) {
    testLyricsBtn.addEventListener('click', function() {
      testEmote('lyricsEmoji', lyricsInput.value);
    });
  }

  var saveBtn = document.getElementById('btn-save-emotes');
  if (saveBtn) saveBtn.addEventListener('click', saveEmoteSettings);

  var revertBtn = document.getElementById('btn-revert-emotes');
  if (revertBtn) revertBtn.addEventListener('click', revertEmotes);
}

function initTokenSettings() {
  var saveTokenBtn = document.getElementById('btn-save-discord-token');
  if (saveTokenBtn) {
    saveTokenBtn.addEventListener('click', saveDiscordToken);
  }
  var tokenInput = document.getElementById('opt-discord-token');
  if (tokenInput) {
    tokenInput.addEventListener('keydown', function(e) {
      if (e.key === 'Enter') {
        e.preventDefault();
        saveDiscordToken();
      }
    });
  }
}

if (syncLeadSlider && syncLeadValue) {
  syncLeadSlider.addEventListener('input', function() {
    var val = parseInt(this.value, 10);
    syncLeadValue.textContent = val + 'ms';
    try { localStorage.setItem('discord-lyrics-sync-lead', val); } catch {}

    // Envia para o backend em tempo real
    fetch('/api/settings', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ syncLeadMs: val }),
    }).catch(function(err) {
      console.error('[Settings] Failed to sync lead:', err);
    });

    // Ajuste manual desativa o adaptive sync (otimista — o backend confirma
    // via sync-metrics/state, mas a UI reage imediatamente)
    if (state.adaptiveSyncEnabled) {
      state.adaptiveSyncEnabled = false;
      updateSyncMetrics();
    }
  });
}

// ── Reativar ajuste automático do lead (adaptive sync) ───────────────────
var btnReenableAdaptive = document.getElementById('btn-reenable-adaptive');
if (btnReenableAdaptive) {
  btnReenableAdaptive.addEventListener('click', function() {
    btnReenableAdaptive.disabled = true;
    fetch('/api/settings', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adaptiveSync: true }),
    })
      .then(function() {
        // Otimista — o backend confirma via sync-metrics/state com o novo lead
        state.adaptiveSyncEnabled = true;
        updateSyncMetrics();
      })
      .catch(function(err) {
        console.error('[Settings] Failed to re-enable adaptive sync:', err);
      })
      .finally(function() {
        btnReenableAdaptive.disabled = false;
      });
  });
}

// ── Seek: Click/Drag on Progress Bar ─────────────────────────────────────────
let isSeeking = false;
/** Último texto de tempo escrito no DOM (guard do tick — compartilhado com doSeek/pause/reset) */
let _lastTimeText = '';
/** Metade da largura do tooltip, cacheada para evitar leitura de layout no hot path */
let _tooltipHalfWidth = 0;

/**
 * Calcula a posição do mouse/touch como fração (0..1) no progress bar.
 */
function getClickFraction(e) {
  const rect = els.progressBarBg.getBoundingClientRect();
  // No touchend o `touches` é uma TouchList VAZIA (truthy) — usa changedTouches.
  const touch = (e.touches && e.touches.length) ? e.touches[0]
    : (e.changedTouches && e.changedTouches.length) ? e.changedTouches[0]
    : null;
  const clientX = touch ? touch.clientX : e.clientX;
  return Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
}

/**
 * Atualiza hover indicator + tooltip durante o hover/drag.
 */
function updateSeekPreview(fraction) {
  if (!state.durationMs || state.durationMs <= 0) return;
  const positionMs = fraction * state.durationMs;
  if (els.progressHover) {
    setHoverPct(fraction * 100);
  }
  if (els.progressTooltip) {
    els.progressTooltip.textContent = formatTime(positionMs);
    // Cacheia a metade da largura do tooltip: com tabular-nums ela é constante,
    // e ler offsetWidth a cada mousemove/drag seria layout thrash no hot path.
    if (!_tooltipHalfWidth) _tooltipHalfWidth = els.progressTooltip.offsetWidth / 2 || 20;
    els.progressTooltip.style.left = 'calc(' + (fraction * 100) + '% - ' + _tooltipHalfWidth + 'px)';
  }
}

/**
 * Cria animação de ondulação (ripple) na posição do clique.
 */
function createRipple(fraction) {
  if (!els.progressBarBg) return;
  const ripple = document.createElement('div');
  ripple.className = 'ripple';
  ripple.style.left = (fraction * 100) + '%';

  const inner = document.createElement('div');
  inner.className = 'ripple-inner';
  ripple.appendChild(inner);

  els.progressBarBg.appendChild(ripple);

  // Remove após animação
  setTimeout(function() {
    if (ripple.parentNode) ripple.parentNode.removeChild(ripple);
  }, 700);
}

/**
 * Executa o seek no backend e atualiza o state local imediatamente.
 */
async function doSeek(fraction, fromMouseEvent) {
  if (!state.durationMs || state.durationMs <= 0) return;
  const positionMs = Math.floor(fraction * state.durationMs);

  // Cria ripple no local do clique
  if (fromMouseEvent) {
    createRipple(fraction);
  }

  // Atualiza local imediatamente para feedback responsivo
  state.progressMs = positionMs;
  state.lastProgressTime = performance.now();
  // Seek intencional do usuário: cancela qualquer reajuste pendente (barra e letras)
  state.smoothMs = positionMs;
  state.resyncActive = false;
  cancelLyricGlide();
  // A barra pula instantaneamente no seek — as letras devem acompanhar igual:
  // com currentLyricIndex = -1, o próximo lyric-line/state aplica o índice
  // direto (sem flash pelas linhas intermediárias). O glide fica reservado
  // para divergência NÃO intencional (stall, reconexão, seek externo).
  state.currentLyricIndex = -1;
  updateProgressSyncStatus();
  const pct = (positionMs / state.durationMs) * 100;
  setProgressPct(pct);
  _lastTimeText = formatTime(positionMs);
  els.timeCurrent.textContent = _lastTimeText;
  setArtRingProgress(pct);

  // Feedback visual: destaca a barra
  if (els.progressBarBg) els.progressBarBg.classList.add('seeking');
  setTimeout(function() {
    if (els.progressBarBg) els.progressBarBg.classList.remove('seeking');
  }, 300);

  // Envia seek para o backend via SMTC
  try {
    const res = await fetch('/api/seek', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ positionMs }),
    });
    if (!res.ok) {
      console.warn('[Seek] Backend returned', res.status);
    }
  } catch (err) {
    console.warn('[Seek] Failed:', err.message);
  }
}

// ── Mouse events ────────────────────────────────────────────────────────
if (els.progressBarBg) {
  // Hover — mostra preview da posição
  els.progressBarBg.addEventListener('mousemove', function(e) {
    if (!state.durationMs || state.durationMs <= 0) return;
    els.progressBarBg.classList.add('hovering');
    const fraction = getClickFraction(e);
    updateSeekPreview(fraction);
  });

  els.progressBarBg.addEventListener('mouseleave', function() {
    els.progressBarBg.classList.remove('hovering');
    if (!isSeeking && els.progressHover) {
      setHoverPct(0);
    }
  });

  // Click/Drag — mousedown + mousemove + mouseup cobre tanto cliques quanto drags
  // (click event não é usado porque ele dispara DEPOIS do mouseup, causando double ripple)
  els.progressBarBg.addEventListener('mousedown', function(e) {
    // Seek permitido mesmo pausado (consistente com as setas ←/→ do teclado)
    if (!state.durationMs || state.durationMs <= 0) return;
    isSeeking = true;
    const fraction = getClickFraction(e);
    updateSeekPreview(fraction);
    els.progressBarBg.classList.add('seeking');

    // Atualiza durante o drag via RAF
    function onMouseMove(ev) {
      if (!isSeeking) return;
      const f = getClickFraction(ev);
      updateSeekPreview(f);
    }

    function onMouseUp(ev) {
      if (!isSeeking) return;
      isSeeking = false;
      const f = getClickFraction(ev);
      doSeek(f, true);
      els.progressBarBg.classList.remove('seeking');
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
    }

    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
  });

  // Touch events para mobile
  els.progressBarBg.addEventListener('touchstart', function(e) {
    if (!state.durationMs || state.durationMs <= 0) return;
    isSeeking = true;
    const fraction = getClickFraction(e);
    updateSeekPreview(fraction);
    els.progressBarBg.classList.add('seeking');
  }, { passive: true });

  els.progressBarBg.addEventListener('touchmove', function(e) {
    if (!isSeeking) return;
    const fraction = getClickFraction(e);
    updateSeekPreview(fraction);
  }, { passive: true });

  els.progressBarBg.addEventListener('touchend', function(e) {
    if (!isSeeking) return;
    isSeeking = false;
    const fraction = getClickFraction(e);
    doSeek(fraction, true);
    els.progressBarBg.classList.remove('seeking');
  });

  // Toque cancelado pelo SO (gesto, notificação, etc.) — limpa o estado de drag
  // para não deixar `isSeeking`/classe `seeking` presos até a próxima interação.
  els.progressBarBg.addEventListener('touchcancel', function() {
    if (!isSeeking) return;
    isSeeking = false;
    els.progressBarBg.classList.remove('seeking');
  });
}

// ── Sobre: versão/plataforma (sem auto-updater) ─────────────────────────────
async function initAboutInfo() {
  if (!window.electronAPI) return;
  try {
    const version = await window.electronAPI.getAppVersion();
    var verEl = $('#about-version');
    if (verEl) verEl.textContent = version;
    var platEl = $('#about-platform');
    if (platEl) platEl.textContent = 'Electron';
  } catch {}
}

// ── Desempenho: efeitos de fundo + modo econômico ───────────────────────────
/**
 * Lê as preferências salvas (localStorage) e aplica os estados visuais.
 * Chamado antes de iniciar as partículas (para não spawnar se fx-off).
 */
function applySavedPerfSettings() {
  var fxOff = false;
  var eco = false;
  var reduceMotion = false;
  var noBlur = false;
  var ambient = false;
  var density = 10;
  var logLimit = 200;
  try {
    fxOff = localStorage.getItem('dls-fx-off') === '1';
    eco = localStorage.getItem('dls-eco') === '1';
    reduceMotion = localStorage.getItem('dls-reduce-motion') === '1';
    noBlur = localStorage.getItem('dls-no-blur') === '1';
    ambient = localStorage.getItem('dls-ambient') === '1';
    var d = parseInt(localStorage.getItem('dls-particle-density') || '', 10);
    if (!isNaN(d)) density = Math.max(0, Math.min(30, d));
    var l = parseInt(localStorage.getItem('dls-log-limit') || '', 10);
    if (!isNaN(l)) logLimit = Math.max(20, Math.min(500, l));
  } catch {}

  document.body.classList.toggle('fx-off', fxOff);
  document.body.classList.toggle('eco-mode', eco);
  document.body.classList.toggle('reduce-motion', reduceMotion);
  document.body.classList.toggle('no-blur', noBlur);
  document.body.classList.toggle('ambient-on', ambient);

  // Aplica a densidade de partículas salva (antes do init/start das partículas)
  if (particleSystem && typeof particleSystem.setDensity === 'function') {
    particleSystem.setDensity(density);
  }
  LOG_LIMIT = logLimit;

  // Sincroniza os toggles/slider/select do drawer de configurações (se já montados)
  var fxToggle = document.getElementById('opt-fx-effects');
  if (fxToggle) fxToggle.checked = !fxOff;
  var ecoToggle = document.getElementById('opt-eco-mode');
  if (ecoToggle) ecoToggle.checked = eco;
  var rmToggle = document.getElementById('opt-reduce-motion');
  if (rmToggle) rmToggle.checked = reduceMotion;
  var nbToggle = document.getElementById('opt-no-blur');
  if (nbToggle) nbToggle.checked = noBlur;
  var ambToggle = document.getElementById('opt-ambient-mode');
  if (ambToggle) ambToggle.checked = ambient;
  // Modo ambiente: aplica a capa atual assim que o toggle é ligado
  if (ambient) updateAmbientBg(state.albumArtUrl);

  // Fechar para a bandeja: sincroniza o toggle com a preferência do main process
  // (persistida no config.json compartilhado). Ajuste manual do usuário envia
  // via IPC (set-close-to-tray) — aqui só refletimos o valor salvo no startup.
  var ctToggle = document.getElementById('opt-close-to-tray');
  if (ctToggle && window.electronAPI && window.electronAPI.getCloseToTray) {
    window.electronAPI.getCloseToTray().then(function(v) {
      ctToggle.checked = v !== false;
    }).catch(function() {});
  }
  var densitySlider = document.getElementById('opt-particle-density');
  if (densitySlider) densitySlider.value = density;
  var densityValue = document.getElementById('opt-particle-density-value');
  if (densityValue) densityValue.textContent = String(density);
  var logSelect = document.getElementById('opt-log-limit');
  if (logSelect) logSelect.value = String(logLimit);
  // Limpa entradas do log que excedem o novo limite (memória imediata)
  var lc = document.getElementById('log-container');
  if (lc) {
    while (lc.children.length > LOG_LIMIT) lc.removeChild(lc.firstChild);
    updateLogCount();
  }
}

/**
 * Aplica a preferência de efeitos de fundo (partículas/orbes).
 * @param {boolean} enabled
 */
function applyFxEffects(enabled) {
  document.body.classList.toggle('fx-off', !enabled);
  try { localStorage.setItem('dls-fx-off', enabled ? '0' : '1'); } catch {}

  if (enabled) {
    if (particleSystem && !particleSystem.running) {
      particleSystem.init();
      particleSystem.start();
      // Eco mode ligado: pausa logo apos o start (start() zera paused)
      if (document.body.classList.contains('eco-mode')) {
        particleSystem.pause();
      }
    }
  } else {
    if (particleSystem && particleSystem.running) {
      particleSystem.stop();
    }
  }
}

/**
 * Aplica "Reduzir animações" (congela animações CSS caras em GPU).
 * @param {boolean} enabled
 */
function applyReduceMotion(enabled) {
  document.body.classList.toggle('reduce-motion', enabled);
  try { localStorage.setItem('dls-reduce-motion', enabled ? '1' : '0'); } catch {}
}

/**
 * Aplica "Sem desfoque de fundo" (remove blur/backdrop-filter, caros em GPU).
 * @param {boolean} enabled
 */
function applyNoBlur(enabled) {
  document.body.classList.toggle('no-blur', enabled);
  try { localStorage.setItem('dls-no-blur', enabled ? '1' : '0'); } catch {}
}

/**
 * Aplica o modo ambiente (capa do álbum como fundo da janela inteira).
 * @param {boolean} enabled
 */
function applyAmbientMode(enabled) {
  document.body.classList.toggle('ambient-on', enabled);
  try { localStorage.setItem('dls-ambient', enabled ? '1' : '0'); } catch {}
  // Sincroniza o fundo com a capa atual (ou limpa se desligado)
  updateAmbientBg(enabled ? state.albumArtUrl : null);
}

/**
 * Aplica a densidade de partículas (0..30) em tempo real.
 * @param {number} n
 */
function applyParticleDensity(n) {
  var count = Math.max(0, Math.min(30, Math.round(Number(n) || 0)));
  try { localStorage.setItem('dls-particle-density', String(count)); } catch {}
  if (particleSystem && typeof particleSystem.setDensity === 'function') {
    particleSystem.setDensity(count);
  }
  var densityValue = document.getElementById('opt-particle-density-value');
  if (densityValue) densityValue.textContent = String(count);
}

/**
 * Aplica o limite de entradas do Activity Log.
 * @param {number} limit
 */
function applyLogLimit(limit) {
  var n = Math.max(20, Math.min(500, Math.round(Number(limit) || 200)));
  LOG_LIMIT = n;
  try { localStorage.setItem('dls-log-limit', String(n)); } catch {}
  var lc = document.getElementById('log-container');
  if (lc) {
    while (lc.children.length > LOG_LIMIT) lc.removeChild(lc.firstChild);
    updateLogCount();
  }
}

/**
 * Aplica o modo econômico e sincroniza com o backend (poll interval).
 * @param {boolean} enabled
 */
function applyEcoMode(enabled) {
  document.body.classList.toggle('eco-mode', enabled);
  try { localStorage.setItem('dls-eco', enabled ? '1' : '0'); } catch {}

  // Eco mode: pausa de verdade o spawn de partículas (sem isso o timer
  // continua criando nós no DOM que o CSS apenas esconde)
  if (particleSystem && particleSystem.running) {
    if (enabled) {
      particleSystem.pause();
    } else {
      particleSystem.resume();
    }
  }

  // Informa o backend para ajustar o intervalo de polling
  fetch('/api/settings', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ ecoMode: enabled }),
  }).catch(function(err) {
    console.error('[Settings] Failed to save eco mode:', err);
  });
}

/** Atualiza o indicador de memória em uso (Chromium/Electron apenas). */
function updateMemoryUsage() {
  // Só atualiza quando o drawer de configurações está aberto — o setInterval
  // de 3s não causa reflow nem releitura com o app ocioso.
  var drawer = document.getElementById('settings-drawer');
  if (!drawer || !drawer.classList.contains('open')) return;
  var el = document.getElementById('opt-memory');
  if (!el) return;
  if (!window.performance || !performance.memory) {
    el.textContent = 'n/d';
    return;
  }
  var mb = performance.memory.usedJSHeapSize / 1048576;
  el.textContent = mb.toFixed(1) + ' MB';
}

/**
 * Liga os toggles de Desempenho do drawer de configurações.
 */
function initOptimizationSettings() {
  var fxToggle = document.getElementById('opt-fx-effects');
  if (fxToggle) {
    fxToggle.addEventListener('change', function() {
      applyFxEffects(this.checked);
    });
  }

  var ecoToggle = document.getElementById('opt-eco-mode');
  if (ecoToggle) {
    ecoToggle.addEventListener('change', function() {
      applyEcoMode(this.checked);
    });
  }

  var rmToggle = document.getElementById('opt-reduce-motion');
  if (rmToggle) {
    rmToggle.addEventListener('change', function() {
      applyReduceMotion(this.checked);
    });
  }

  var nbToggle = document.getElementById('opt-no-blur');
  if (nbToggle) {
    nbToggle.addEventListener('change', function() {
      applyNoBlur(this.checked);
    });
  }

  var ambToggle = document.getElementById('opt-ambient-mode');
  if (ambToggle) {
    ambToggle.addEventListener('change', function() {
      applyAmbientMode(this.checked);
    });
  }

  // Fechar para a bandeja: envia a preferência ao main process (que persiste
  // no config.json e decide se o X esconde ou encerra o app).
  var ctToggle = document.getElementById('opt-close-to-tray');
  if (ctToggle) {
    ctToggle.addEventListener('change', function() {
      if (window.electronAPI && window.electronAPI.setCloseToTray) {
        window.electronAPI.setCloseToTray(this.checked);
      }
    });
  }

  var densitySlider = document.getElementById('opt-particle-density');
  if (densitySlider) {
    densitySlider.addEventListener('input', function() {
      applyParticleDensity(this.value);
    });
  }

  var logSelect = document.getElementById('opt-log-limit');
  if (logSelect) {
    logSelect.addEventListener('change', function() {
      applyLogLimit(this.value);
    });
  }

  // Atualiza a memória ao abrir as configurações + a cada 3s (barato)
  updateMemoryUsage();
  setInterval(updateMemoryUsage, 3000);
}

// ── Init ──────────────────────────────────────────────────────────────────────
async function init() {
  // Carrega o tema salvo (uma única vez, sem transição)
  loadSavedTheme();

  // Preenche a seção "Sobre" com versão/plataforma (sem auto-updater)
  initAboutInfo();

  // Conecta o WebSocket IMEDIATAMENTE (antes do fetch) — o backend envia o
  // estado inicial assim que a conexão abre, então a UI popula sozinha.
  // Única chamada de conexão (idempotente) — sem timers duplicados no startup.
  connectWebSocket();

  // Fetch initial state (pintura inicial mais rápida / fallback)
  try {
    const res = await fetch('/api/status');
    if (res.ok) {
      const data = await res.json();
      updateUI(data);
      addLog('info', 'APP', 'Connected to backend');
    } else {
      addLog('error', 'APP', 'Failed to fetch initial state (HTTP ' + res.status + ')');
    }
  } catch (err) {
    addLog('error', 'APP', 'Connection failed: ' + (err.message || 'unknown'));
  }
}

document.addEventListener('DOMContentLoaded', function() {
  // Aguarda um momento para garantir que o servidor está pronto
  setTimeout(init, 500);
});
