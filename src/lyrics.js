// LRCLIB — gratuito, sem chave de API
// Docs: https://lrclib.net/docs
const BASE_URL = 'https://lrclib.net/api';

/** Cache em memória: trackId → { data: lyricsResult | null, timestamp: number } */
const cache = new Map();
const CACHE_TTL_MS = 5 * 60 * 1000; // 5 minutos para letras OK
const NULL_CACHE_TTL_MS = 30 * 1000; // 30 segundos para null (não encontrado)

import * as ui from './ui.js';
import { getGeniusLyrics, distributeUnsyncedLines } from './genius.js';

// ── Analisador LRC ────────────────────────────────────────────────────────────

/**
 * Analisa uma string LRC em um array de objetos { timeMs, text }.
 * Suporta formatos [mm:ss.xx] e [mm:ss.xxx].
 */
function parseLRC(lrc) {
  const lines = [];
  const regex = /^\[(\d{2}):(\d{2})\.(\d{2,3})\]\s*(.*)$/;

  for (const rawLine of lrc.split('\n')) {
    const match = rawLine.trim().match(regex);
    if (!match) continue;

    const [, mm, ss, cs, text] = match;
    const ms =
      parseInt(mm, 10) * 60_000 +
      parseInt(ss, 10) * 1_000 +
      (cs.length === 3 ? parseInt(cs, 10) : parseInt(cs, 10) * 10);

    lines.push({ timeMs: ms, text: text.trim() });
  }

  return lines.sort((a, b) => a.timeMs - b.timeMs);
}

// ── Public API ────────────────────────────────────────────────────────────────

/**
 * Busca e analisa letras sincronizadas do LRCLIB (gratuito, sem chave necessária).
 * Retorna um objeto com { synced, lines } ou null se não encontrado.
 * synced = true  → lines são { timeMs, text } com timestamps reais
 * synced = false → lines são { timeMs, text } com timestamps estimados por distribuição uniforme
 *
 * @param {string} trackId    Spotify/synthetic track ID (for caching only)
 * @param {string} trackName
 * @param {string} artistName
 * @param {string} albumName
 * @param {number} durationMs
 * @returns {Promise<{synced: boolean, lines: Array<{timeMs: number, text: string}>}|null>}
 */
export async function getLyrics(trackId, trackName, artistName, albumName = '', durationMs = 0) {
  // Check cache with TTL — null entries expiram mais rápido
  const cached = cache.get(trackId);
  if (cached) {
    const ttl = cached.data ? CACHE_TTL_MS : NULL_CACHE_TTL_MS;
    if (Date.now() - cached.timestamp < ttl) {
      return cached.data;
    }
    // Cache expirado: remove e tenta de novo
    cache.delete(trackId);
  }

  async function attemptFetch() {
    const params = new URLSearchParams({
      track_name: trackName,
      artist_name: artistName,
    });
    if (albumName) params.set('album_name', albumName);
    if (durationMs > 0) params.set('duration', Math.round(durationMs / 1000).toString());

    const res = await fetch(`${BASE_URL}/get?${params}`, {
      headers: {
        'User-Agent': 'DiscordLyricsStatus/1.0.0 (github.com/rafaelacat/discord-status-spotify)',
        'Accept': 'application/json',
      },
    });

    if (res.status === 404) {
      return { notFound: true };
    }

    if (!res.ok) {
      throw new Error(`LRCLIB HTTP ${res.status}`);
    }

    const data = await res.json();

    if (!data?.syncedLyrics) {
      return { notFound: true };
    }

    const lines = parseLRC(data.syncedLyrics);
    return { synced: true, lines };
  }

  try {
    const result = await attemptFetch();

    if (result.notFound) {
      // Tenta busca alternativa no LRCLIB via /search
      const altResult = await attemptAlternative(trackName, artistName, durationMs);
      if (altResult) {
        ui.lyricsInfo(trackName, altResult.length);
        const resultData = { synced: true, lines: altResult };
        cache.set(trackId, { data: resultData, timestamp: Date.now() });
        return resultData;
      }

      // Fallback FINAL: Genius (letras NÃO sincronizadas)
      // Só tenta se tiver duração para distribuir os timestamps
      if (durationMs > 10000) {
        const geniusLines = await getGeniusLyrics(trackName, artistName);
        if (geniusLines && geniusLines.length >= 3) {
          const distributed = distributeUnsyncedLines(geniusLines, durationMs);
          ui.lyricsInfo(trackName, distributed.length, 'Genius (unsynced)');
          const result = { synced: false, lines: distributed };
          cache.set(trackId, { data: result, timestamp: Date.now() });
          return result;
        }
      }

      ui.lyricsNotFound(trackName, artistName);
      cache.set(trackId, { data: null, timestamp: Date.now() });
      return null;
    }

    const { synced, lines } = result;
    ui.lyricsInfo(trackName, lines.length);
    const resultData = { synced, lines };
    cache.set(trackId, { data: resultData, timestamp: Date.now() });
    return resultData;
  } catch (err) {
    ui.fetchError('Letras', err.message);
    cache.set(trackId, { data: null, timestamp: Date.now() });
    return null;
  }
}

/**
 * Tenta busca alternativa no LRCLIB via endpoint /search.
 * Usa o nome da música sem caracteres especiais e correspondência por duração.
 */
async function attemptAlternative(trackName, artistName, durationMs) {
  try {
    // Busca por search (mais tolerante que o get por query params)
    const searchQuery = encodeURIComponent(`${trackName} ${artistName}`.substring(0, 100));
    const searchRes = await fetch(`${BASE_URL}/search?q=${searchQuery}`, {
      headers: {
        'User-Agent': 'DiscordLyricsStatus/1.0.0',
        'Accept': 'application/json',
      },
    });

    if (!searchRes.ok) return null;
    const results = await searchRes.json();
    if (!Array.isArray(results) || results.length === 0) return null;

    // Filtra por artista + duração aproximada
    const artistLower = artistName.toLowerCase();
    const durationSec = durationMs > 0 ? Math.round(durationMs / 1000) : 0;

    for (const item of results) {
      const artistMatch = item.artistName?.toLowerCase().includes(artistLower)
        || artistLower.includes((item.artistName || '').toLowerCase());

      if (!artistMatch) continue;

      // Se tiver duração, verifica se coincide (±3s)
      if (durationSec > 0 && item.duration) {
        const diff = Math.abs(item.duration - durationSec);
        if (diff > 3) continue; // Duração muito diferente, não é a mesma música
      }

      // Encontrou! Busca a letra completa
      const detailRes = await fetch(`${BASE_URL}/get/${item.id}`, {
        headers: {
          'User-Agent': 'DiscordLyricsStatus/1.0.0',
          'Accept': 'application/json',
        },
      });

      if (!detailRes.ok) continue;
      const detail = await detailRes.json();

      if (detail?.syncedLyrics) {
        const lines = parseLRC(detail.syncedLyrics);
        if (lines.length > 0) {
          return lines;
        }
      }
    }

    return null;
  } catch {
    return null;
  }
}
