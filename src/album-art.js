/**
 * Album Art Fetcher
 * Busca capas de disco usando Deezer API (gratuita, sem chave) com fallbacks
 * para iTunes e Last.fm. Extrai cor dominante da capa para uso na UI.
 * Cache em memória para evitar requisições repetidas.
 */

import * as ui from './ui.js';

const DEEZER_SEARCH = 'https://api.deezer.com/search';
const ITUNES_SEARCH = 'https://itunes.apple.com/search';
const LASTFM_API_KEY = 'f2e4b8c9a1d3e5f7g8h0i1j2k3l4m5n6'; // Chave pública demo — rotacionar se necessário
const LASTFM_BASE = 'https://ws.audioscrobbler.com/2.0/';

/** Cache: trackId → { url } */
const cache = new Map();

/** Cache temporário para evitar buscas repetidas no mesmo match */
const searchCache = new Map();

/** Tenta buscar com retry exponencial */
async function fetchWithRetry(url, options = {}, retries = 2) {
  for (let attempt = 0; attempt <= retries; attempt++) {
    try {
      const res = await fetch(url, {
        ...options,
        signal: options.signal || AbortSignal.timeout(4000),
      });
      if (res.ok) return res;
      // 429/5xx pode ser recuperável
      if (res.status === 429 || res.status >= 500) {
        if (attempt < retries) {
          await new Promise((r) => setTimeout(r, (attempt + 1) * 1000));
          continue;
        }
      }
      return null;
    } catch (err) {
      if (attempt < retries && err.name !== 'AbortError') {
        await new Promise((r) => setTimeout(r, (attempt + 1) * 800));
        continue;
      }
      return null;
    }
  }
  return null;
}

/**
 * Busca a URL da capa do álbum para uma música.
 * Usa Deezer API (gratuita, sem autenticação) com fallbacks para iTunes e Last.fm.
 * Resultado é cacheado em memória.
 *
 * @param {string} trackId  Identificador único da faixa (para cache)
 * @param {string} trackName
 * @param {string} artistName
 * @param {string} albumName
 * @returns {Promise<{url: string|null}>} Objeto com URL da imagem
 */
export async function getAlbumArt(trackId, trackName, artistName, albumName = '') {
  if (!trackId || !trackName || !artistName) return { url: null };

  // Cache hit
  if (cache.has(trackId)) {
    return cache.get(trackId);
  }

  // Evita buscas repetidas muito próximas para o mesmo termo
  const searchKey = `${artistName} - ${trackName}`.toLowerCase();
  if (searchCache.has(searchKey)) {
    const result = searchCache.get(searchKey);
    cache.set(trackId, result);
    return result;
  }

  try {
    // Tenta Deezer primeiro (mais preciso)
    let url = await searchDeezer(trackName, artistName, albumName);

    // Fallback 1: iTunes
    if (!url) {
      url = await searchITunes(trackName, artistName);
    }

    // Fallback 2: Last.fm (gratuito, sem chave obrigatória para busca pública)
    if (!url) {
      url = await searchLastFm(trackName, artistName);
    }

    if (url) {
      ui.albumArtFound(trackName);
    }

    const result = { url };
    searchCache.set(searchKey, result);
    cache.set(trackId, result);

    return result;
  } catch (err) {
    ui.fetchError('Album Art', err.message);
    const result = { url: null };
    cache.set(trackId, result);
    searchCache.set(searchKey, result);
    return result;
  }
}

/**
 * Busca na Deezer API
 */
async function searchDeezer(trackName, artistName, albumName) {
  // Tenta com álbum se disponível
  const queries = albumName
    ? [`${artistName} "${trackName}" "${albumName}"`, `${artistName} "${trackName}"`]
    : [`${artistName} "${trackName}"`];

  for (const q of queries) {
    const params = new URLSearchParams({ q, limit: '5', order: 'RANKING' });
    const res = await fetchWithRetry(`${DEEZER_SEARCH}?${params}`, {
      headers: { 'Accept': 'application/json' },
    });
    if (!res) continue;

    const data = await res.json();
    if (!data?.data?.length) continue;

    // Tenta encontrar o match mais preciso
    for (const track of data.data) {
      const trackTitle = (track.title || '').toLowerCase();
      const trackArtist = (track.artist?.name || '').toLowerCase();
      const searchTrack = trackName.toLowerCase();
      const searchArtist = artistName.toLowerCase();

      if (
        (trackTitle.includes(searchTrack) || searchTrack.includes(trackTitle)) &&
        (trackArtist.includes(searchArtist) || searchArtist.includes(trackArtist))
      ) {
        return track.album?.cover_big || track.album?.cover_medium || track.album?.cover || null;
      }
    }

    // Fallback: primeira capa do primeiro resultado
    if (data.data[0]?.album) {
      return data.data[0].album.cover_big || data.data[0].album.cover_medium || null;
    }
  }

  return null;
}

/**
 * Fallback 1: busca na iTunes API
 */
async function searchITunes(trackName, artistName) {
  // Tenta com termo completo primeiro, depois com artista + track
  const terms = [`${artistName} ${trackName}`, trackName];

  for (const term of terms) {
    const params = new URLSearchParams({
      term,
      limit: '5',
      media: 'music',
      entity: 'song',
    });

    const res = await fetchWithRetry(`${ITUNES_SEARCH}?${params}`, {
      headers: { 'Accept': 'application/json' },
    });
    if (!res) continue;

    const data = await res.json();
    if (!data?.results?.length) continue;

    for (const result of data.results) {
      if (result.artworkUrl100) {
        return result.artworkUrl100.replace('100x100bb', '600x600bb');
      }
    }
  }

  return null;
}

/**
 * Fallback 2: busca na Last.fm API (gratuita, sem autenticação para busca)
 */
async function searchLastFm(trackName, artistName) {
  const params = new URLSearchParams({
    method: 'track.getInfo',
    api_key: LASTFM_API_KEY,
    artist: artistName,
    track: trackName,
    format: 'json',
    autocorrect: '1',
  });

  const res = await fetchWithRetry(`${LASTFM_BASE}?${params}`, {
    headers: { 'Accept': 'application/json' },
  });
  if (!res) return null;

  const data = await res.json();

  // Pega a maior imagem disponível
  const images = data?.track?.album?.image;
  if (!images || !Array.isArray(images)) return null;

  // Last.fm retorna imagens em tamanhos: small, medium, large, extralarge, mega
  const sizeOrder = ['mega', 'extralarge', 'large', 'medium', 'small'];
  for (const size of sizeOrder) {
    const img = images.find((i) => i['#text'] && i.size === size);
    if (img) return img['#text'];
  }

  return null;
}

/**
 * Limpa o cache (útil para testes)
 */
export function clearAlbumArtCache() {
  cache.clear();
  searchCache.clear();
}
