import { getToken } from './config.js';
import * as ui from './ui.js';

const API = 'https://discord.com/api/v9/users/@me/settings';

let lastText = null;
/** Último emoji enviado — o guard compara texto E emoji para que trocar o
 *  emote na config re-dispara o mesmo status com o novo emoji. */
let lastEmoji = null;

function parseEmoji(input) {
  if (!input) return { name: 'musical_note', id: null };
  // Emote customizado do Discord: <:nome:id> (estático) ou <a:nome:id> (animado/Nitro)
  const match = input.match(/<(a)?:(.+?):(\d+)>/);
  if (match) {
    // O prefixo 'a' (animado) NÃO vai no payload — a API decide pelo emoji_id
    return { name: match[2], id: match[3] };
  }
  // Fallback: if it's a plain unicode emoji, use it as emoji_name
  // The Discord API accepts unicode emoji characters directly via emoji_name
  if (input.length <= 2 && /\p{Emoji}/u.test(input)) {
    return { name: input, id: null };
  }
  return { name: 'musical_note', id: null };
}

// ── Adaptive Sync — Latência da API do Discord ────────────────────────────
// Usa um filtro EWMA (Exponentially Weighted Moving Average) para suavizar
// as medições de round-trip time da API do Discord.
//
// EWMA: observações recentes têm mais peso que observações antigas.
// Fator α = 0.3 significa que a última observação contribui com 30%
// e a média histórica com 70%. Isso evita oscilações bruscas.

/** Fator de suavização EWMA — 0.3 = resposta rápida sem oscilar */
const EWMA_ALPHA = 0.3;
/** Latência média estimada em ms (inicia em 200ms — valor padrão do syncLeadMs) */
let estimatedLatencyMs = 200;
/** Número de amostras coletadas para diagnóstico */
let latencySampleCount = 0;
/** Callback chamado quando a latência estimada muda significativamente */
let _onLatencyChange = null;

/**
 * Registra um callback que será chamado com a latência estimada atualizada.
 * @param {(latencyMs: number) => void} handler
 */
export function onDiscordLatencyChange(handler) {
  _onLatencyChange = handler;
}

/**
 * Retorna a latência estimada atual da API do Discord em ms.
 * @returns {number}
 */
export function getEstimatedLatencyMs() {
  return Math.round(estimatedLatencyMs);
}

/** Último valor notificado ao callback — evita disparos repetidos */
let lastNotifiedMs = -1;

/**
 * Registra a latência de uma requisição PATCH e atualiza a média EWMA.
 * @param {number} roundTripMs - Tempo total da requisição em ms
 */
function recordLatency(roundTripMs) {
  // Ignora outliers extremos (> 10s — provavelmente rate limit ou timeout)
  if (roundTripMs > 10000 || roundTripMs < 0) return;

  latencySampleCount++;

  // EWMA: nova_estimativa = α * observação + (1-α) * estimativa_anterior
  estimatedLatencyMs = EWMA_ALPHA * roundTripMs + (1 - EWMA_ALPHA) * estimatedLatencyMs;

  // Notifica o callback quando:
  // - Tiver pelo menos 3 amostras (período de aquecimento)
  // - A mudança for >= 20ms (evita micro-ajustes)
  if (latencySampleCount >= 3 && _onLatencyChange) {
    const rounded = Math.round(estimatedLatencyMs);
    if (Math.abs(rounded - lastNotifiedMs) >= 20) {
      lastNotifiedMs = rounded;
      _onLatencyChange(rounded);
    }
  }
}

// ── Fila ─────────────────────────────────────────────────────────────────────
// Cada entrada: { body, label }
// Worker envia uma de cada vez; em 429 aguarda e repete a MESMA entrada.
// Novas entradas sempre vão para o final — nenhuma letra é perdida.

const queue = [];
let processing = false;

async function processQueue() {
  if (processing) return;
  processing = true;

  // Lê token uma vez no início do batch (pode ter sido configurado durante a fila)
  const token = getToken();
  if (!token) {
    // Esvazia fila se não tem token
    queue.length = 0;
    processing = false;
    return;
  }

  const headers = {
    Authorization: token,
    'Content-Type': 'application/json',
    'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36',
  };

  while (queue.length > 0) {
    const entry = queue[0];
    try {
      // ── Mede latência desde o ENQUEUE (inclui espera na fila) ────
      // O adaptive sync compensa o tempo TOTAL que a letra leva para
      // chegar ao Discord (espera na fila + RTT do PATCH), não só o RTT.
      const reqStart = entry.enqueuedAt ?? performance.now();

      const res = await fetch(API, {
        method: 'PATCH',
        headers,
        body: entry.body,
      });

      const roundTripMs = performance.now() - reqStart;

      if (res.status === 429) {
        // Rate limited — não conta esta latência (é artificial)
        const data = await res.json().catch(() => ({}));
        const wait = Math.ceil((data.retry_after ?? 1) * 1000);
        ui.rateLimited(wait);
        await new Promise((r) => setTimeout(r, wait));
        // Reinicia a medição após o retry_after — não conta a espera
        // artificial do rate limit como latência do PATCH.
        entry.enqueuedAt = performance.now();
        continue;
      }

      // Registra a latência apenas para respostas bem-sucedidas
      // (respostas de erro também têm latência válida)
      recordLatency(roundTripMs);

      if (!res.ok) {
        const text = await res.text().catch(() => '');
        ui.statusError(res.status, text);
      } else {
        if (shouldEmitStatusUpdated(entry.label)) ui.statusUpdated(entry.label);
      }

    } catch (err) {
      // Erro de rede — não registra latência (seria enganoso)
      ui.fetchError('Status', err.message);
    }

    queue.shift();
  }

  processing = false;
}

// ── API Pública ────────────────────────────────────────────────────────────────

let lastStatusUpdatedAt = 0;
let lastStatusLabel = null;

function shouldEmitStatusUpdated(label) {
  const now = Date.now();
  if (lastStatusLabel === label && now - lastStatusUpdatedAt < 1500) return false;
  if (now - lastStatusUpdatedAt < 800) return false;
  lastStatusLabel = label;
  lastStatusUpdatedAt = now;
  return true;
}

export function setCustomStatus(text, emoji = 'musical_note') {
  const token = getToken();
  if (!token) return;
  if (text === lastText && emoji === lastEmoji) return;

  lastText = text;
  lastEmoji = emoji;
  const { name: emojiName, id: emojiId } = parseEmoji(emoji);
  const statusText = text.slice(0, 128);

  // Pré-serializa o body para evitar JSON.stringify na fila
  const body = JSON.stringify({
    custom_status: {
      text: statusText,
      ...(emojiName ? { emoji_name: emojiName } : {}),
      ...(emojiId ? { emoji_id: emojiId } : {}),
      expires_at: null,
    },
  });

  queue.push({ body, label: text, enqueuedAt: performance.now() });
  processQueue();
}


export function clearCustomStatus() {
  const token = getToken();
  if (!token) return;
  if (lastText === null) return;

  lastText = null;
  lastEmoji = null;
  queue.push({ body: '{"custom_status":null}', label: '(clear)', enqueuedAt: performance.now() });
  processQueue();
  ui.statusCleared();
}

export function isStatusEnabled() {
  return !!getToken();
}
