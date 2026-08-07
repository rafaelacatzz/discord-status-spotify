/**
 * Audio Preview Fetcher
 * Busca URL de preview de 30 segundos de múltiplas fontes:
 * 1. Deezer API (primária — já funcionava)
 * 2. iTunes Search API (fallback — sem auth, previewUrl em .m4a)
 * 3. YouTube via invidious API (último recurso)
 * Cache em memória para evitar requisições repetidas.
 */

import * as ui from './ui.js';

const DEEZER_SEARCH = 'https://api.deezer.com/search';
const ITUNES_SEARCH = 'https://itunes.apple.com/search';

/** Cache: trackId → previewUrl | null */
const cache = new Map();

/** Cache temporário para evitar buscas repetidas no mesmo match */
const searchCache = new Map();

/**
 * Busca a URL de preview de 30s para uma música, tentando múltiplas fontes.
 * Ordem: Deezer → iTunes → YouTube
 * Resultado é cacheado em memória.
 *
 * @param {string} trackId  Identificador único da faixa (para cache)
 * @param {string} trackName
 * @param {string} artistName
 * @returns {Promise<string|null>} URL do preview ou null
 */
export async function getPreviewUrl(trackId, trackName, artistName) {
  if (!trackId || !trackName || !artistName) return null;

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
    // Tenta fontes em ordem, para na primeira que retornar URL
    let url = null;

    // 1. Deezer (primária)
    url = await searchDeezerPreview(trackName, artistName);

    // 2. iTunes (fallback)
    if (!url) {
      url = await searchItunesPreview(trackName, artistName);
    }

    // 3. YouTube via invidious (último recurso — busca vídeo relacionado)
    if (!url) {
      url = await searchYoutubePreview(trackName, artistName);
    }

    searchCache.set(searchKey, url);
    cache.set(trackId, url);

    if (url) {
      console.log(`[Preview] Found via ${url.includes('deezer') ? 'Deezer' : url.includes('apple') ? 'iTunes' : 'YouTube'} for ${trackName}`);
    }

    return url;
  } catch (err) {
    console.warn('[Preview] All sources failed:', err.message);
    cache.set(trackId, null);
    searchCache.set(searchKey, null);
    return null;
  }
}

/**
 * Busca o preview na Deezer API
 */
async function searchDeezerPreview(trackName, artistName) {
  const params = new URLSearchParams({
    q: `${artistName} "${trackName}"`,
    limit: '5',
    order: 'RANKING',
  });

  const res = await fetch(`${DEEZER_SEARCH}?${params}`, {
    headers: { 'Accept': 'application/json' },
    signal: AbortSignal.timeout(3000),
  });

  if (!res.ok) return null;

  const data = await res.json();
  if (!data?.data?.length) return null;

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
      return track.preview || null;
    }
  }

  // Fallback: primeiro resultado
  return data.data[0]?.preview || null;
}

/**
 * Busca o preview na iTunes Search API (Apple).
 * Retorna previewUrl em .m4a (30 segundos).
 * API pública, sem necessidade de autenticação.
 */
async function searchItunesPreview(trackName, artistName) {
  const params = new URLSearchParams({
    term: `${artistName} ${trackName}`,
    media: 'music',
    entity: 'song',
    limit: '5',
    country: 'US',
  });

  const res = await fetch(`${ITUNES_SEARCH}?${params}`, {
    headers: { 'Accept': 'application/json' },
    signal: AbortSignal.timeout(3000),
  });

  if (!res.ok) return null;

  const data = await res.json();
  if (!data?.results?.length) return null;

  // Filtra resultados com previewUrl, tenta match preciso
  for (const track of data.results) {
    if (!track.previewUrl) continue;
    const trackNameNorm = (track.trackName || '').toLowerCase();
    const artistNameNorm = (track.artistName || '').toLowerCase();
    const searchTrack = trackName.toLowerCase();
    const searchArtist = artistName.toLowerCase();

    if (
      (trackNameNorm.includes(searchTrack) || searchTrack.includes(trackNameNorm)) &&
      (artistNameNorm.includes(searchArtist) || searchArtist.includes(artistNameNorm))
    ) {
      return track.previewUrl;
    }
  }

  // Fallback: primeiro resultado com previewUrl
  for (const track of data.results) {
    if (track.previewUrl) return track.previewUrl;
  }

  return null;
}

/**
 * Busca o preview no YouTube via invidious API pública.
 * Extrai o primeiro vídeo relacionado à música.
 * NOTA: invidious é um frontend alternativo do YouTube que pode
 * ficar fora do ar. Servidores públicos podem ser lentos.
 */
async function searchYoutubePreview(trackName, artistName) {
  const YT_SEARCH = `https://music.youtube.com/search?q=${encodeURIComponent(artistName + ' ' + trackName)}`;

  // Usa invidious API para busca de vídeo
  const INVIOUS_INSTANCES = [
    'https://inv.nadeko.net',
    'https://yewtu.be',
    'https://inv.riverside.rocks',
  ];

  for (const instance of INVIOUS_INSTANCES) {
    try {
      const searchUrl = `${instance}/api/v1/search?q=${encodeURIComponent(artistName + ' ' + trackName)}&type=video`;
      const res = await fetch(searchUrl, {
        signal: AbortSignal.timeout(2500),
      });

      if (!res.ok) continue;

      const data = await res.json();
      if (!data || !Array.isArray(data)) continue;

      // Procura o primeiro resultado com videoId
      for (const item of data) {
        if (item.type === 'video' && item.videoId) {
          // Constrói URL de áudio via invidious
          const audioUrl = `${instance}/latest_version?id=${item.videoId}&itag=140`;
          // Testa se o link é acessível
          const headRes = await fetch(audioUrl, {
            method: 'HEAD',
            signal: AbortSignal.timeout(2000),
          });
          if (headRes.ok) {
            return audioUrl;
          }
        }
      }
    } catch {
      // Tenta próxima instância
      continue;
    }
  }

  return null;
}

/**
 * Limpa o cache (útil para testes)
 */
export function clearPreviewCache() {
  cache.clear();
  searchCache.clear();
}
