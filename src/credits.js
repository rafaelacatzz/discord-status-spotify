/**
 * Track Credits Fetcher — busca artistas completos (feats), produtores e
 * escritores de uma faixa via API pública do Genius.
 *
 * O SMTC/Spotify só expõe o artista PRINCIPAL (ex: "Future"), mas a faixa real
 * tem feats e créditos (ex: "Type Shit" → Future, Metro Boomin, Travis Scott &
 * Playboi Carti; produtores: Metro Boomin, MIKE DEAN, D. Rich). Estes créditos
 * são exibidos na UI junto à barra de progresso.
 *
 * Fluxo:
 *   1. GET https://genius.com/api/search/multi?q=<track> <artist>  (JSON)
 *   2. Escolhe o hit mais relevante (score por título + artista, evita traduções)
 *   3. GET https://genius.com/api/songs/<id>  (JSON completo com credits)
 */

const GENIUS_USER_AGENT = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36';
const SEARCH_TIMEOUT_MS = 8000;
const SONG_TIMEOUT_MS = 8000;

/** Normaliza texto para comparação fuzzy (minúsculas, só alfanuméricos). */
function norm(text) {
  return String(text || '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, ' ')
    .trim();
}

/**
 * Cache simples por faixa — evita refetch quando o mesmo track aparece
 * (ex: troca de faixa seguida de pausa da MESMA faixa).
 * Chave: title::artist normalizado.
 */
const creditsCache = new Map();
const CREDITS_CACHE_MAX = 50;

function cacheKey(trackName, artistName) {
  return `${norm(trackName)}::${norm(artistName)}`;
}

/**
 * Busca na API de pesquisa do Genius o hit mais relevante para a faixa.
 * @param {string} trackName
 * @param {string} artistName
 * @returns {Promise<object|null>} resultado do hit (com .id) ou null
 */
async function searchSong(trackName, artistName) {
  const q = encodeURIComponent(`${trackName} ${artistName}`);
  const res = await fetch(`https://genius.com/api/search/multi?q=${q}`, {
    headers: { 'User-Agent': GENIUS_USER_AGENT },
    signal: AbortSignal.timeout(SEARCH_TIMEOUT_MS),
  });
  if (!res.ok) return null;

  const j = await res.json();
  const hits = (j?.response?.sections || []).flatMap((s) => s?.hits || []);
  const candidates = hits
    .map((h) => h?.result)
    .filter((r) => r && r.id && r.lyrics_state === 'complete');

  const tn = norm(trackName);
  const an = norm(artistName);
  let best = null;
  let bestScore = -1;

  for (const r of candidates) {
    let score = 0;
    const t = norm(r.title);
    const ft = norm(r.full_title || '');

    // Título deve conter o nome da música (forte peso)
    if (t.includes(tn) || ft.includes(tn)) score += 3;

    // Sobreposição de artista (peso leve, vários artistas por faixa)
    const artistNames = [
      r.primary_artist?.name,
      ...(r.featured_artists || []).map((a) => a.name),
    ].filter(Boolean);
    for (const nm of artistNames) {
      const nn = norm(nm);
      if (!nn) continue;
      if (an && an.split(' ').some((w) => w.length > 2 && nn.includes(w))) score += 1;
    }

    // Penaliza traduções e remixes/versões alternativas
    if (/türk|tradu|translation|remix|sped up|slowed|lyric video/.test(ft)) score -= 5;

    if (score > bestScore) {
      bestScore = score;
      best = r;
    }
  }

  // Só aceita se o TÍTULO casou (score >= 3) E houver pelo menos uma
  // sobreposição de artista — evita pegar créditos de um cover/faixa homônima
  // de outro artista quando o título é comum.
  if (!best) return null;
  const artistOverlap = [
    best.primary_artist?.name,
    ...(best.featured_artists || []).map((a) => a.name),
  ].some((nm) => {
    const nn = norm(nm);
    return nn && an.split(' ').some((w) => w.length > 2 && nn.includes(w));
  });
  return bestScore >= 3 && artistOverlap ? best : null;
}

/**
 * Busca os créditos completos de uma faixa.
 *
 * @param {string} trackName
 * @param {string} artistName
 * @returns {Promise<object|null>} {
 *   artistNames: string  — lista completa "Future, Metro Boomin, Travis Scott & Playboi Carti"
 *   featured: string[]   — artistas convidados (se o Genius os separar)
 *   producers: string[]  — produtores
 *   writers: string[]    — compositores/escritores
 *   album: string|null
 *   release: string|null — data de lançamento exibível
 * } ou null se não encontrou
 */
export async function getTrackCredits(trackName, artistName) {
  if (!trackName || !artistName) return null;

  const key = cacheKey(trackName, artistName);
  if (creditsCache.has(key)) return creditsCache.get(key);

  try {
    const song = await searchSong(trackName, artistName);
    if (!song?.id) return null;

    const res = await fetch(`https://genius.com/api/songs/${song.id}`, {
      headers: { 'User-Agent': GENIUS_USER_AGENT },
      signal: AbortSignal.timeout(SONG_TIMEOUT_MS),
    });
    if (!res.ok) return null;

    const j = await res.json();
    const s = j?.response?.song;
    if (!s) return null;

    const result = {
      artistNames:
        typeof s.artist_names === 'string' && s.artist_names.trim()
          ? s.artist_names.trim()
          : (s.primary_artist?.name || artistName),
      featured: [...new Set((s.featured_artists || []).map((a) => a.name).filter(Boolean))],
      producers: [...new Set((s.producer_artists || []).map((a) => a.name).filter(Boolean))],
      writers: [...new Set((s.writer_artists || []).map((a) => a.name).filter(Boolean))],
      album: s.album?.name || null,
      release: s.release_date_for_display || null,
    };

    // Sem créditos úteis (nenhum produtor/escritor/convidadop além do artista
    // principal): não cacheia — a próxima faixa igual refaz a busca.
    if (result.producers.length === 0 && result.writers.length === 0 && result.featured.length === 0) {
      return null;
    }

    if (creditsCache.size >= CREDITS_CACHE_MAX) {
      creditsCache.delete(creditsCache.keys().next().value);
    }
    creditsCache.set(key, result);
    return result;
  } catch {
    return null;
  }
}
