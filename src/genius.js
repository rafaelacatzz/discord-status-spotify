/**
 * Genius Lyrics Fetcher — busca letras NÃO sincronizadas do Genius.com.
 * Usado como fallback quando o LRCLIB não encontra letras com timestamps.
 *
 * Fluxo:
 *   1. Constrói URL no formato genius.com/{artist}-{song}-lyrics
 *   2. Faz fetch da página e extrai letras do HTML via regex
 *   3. Retorna array de strings (linhas de texto)
 */

const GENIUS_USER_AGENT = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36';

/**
 * Converte nome de artista/música para slug do Genius.
 * Ex: "Feeling Good" → "feeling-good"
 */
function toSlug(text) {
  return text
    .toLowerCase()
    .replace(/[^\w\s-]/g, '')     // remove pontuação
    .replace(/\s+/g, '-')          // espaços → hífens
    .replace(/-+/g, '-')           // remove hífens duplicados
    .replace(/^-|-$/g, '')         // remove hífens nas bordas
    .substring(0, 80);             // limita tamanho
}

/**
 * Tenta extrair letras de uma página HTML do Genius.
 * Procura por div[data-lyrics-container="true"] e Lyricks__Container.
 */
function extractLyricsFromHtml(html) {
  // Tenta padrão 1: data-lyrics-container (formato novo do Genius)
  const containerRegex = /<div[^>]*data-lyrics-container="true"[^>]*>(.*?)<\/div>/gs;
  let match;
  const parts = [];

  while ((match = containerRegex.exec(html)) !== null) {
    // Remove tags HTML internas (<br>, <i>, etc) e substitui por quebras de linha
    let content = match[1]
      .replace(/<br\s*\/?>/gi, '\n')
      .replace(/<[^>]+>/g, '')
      .replace(/&amp;/g, '&')
      .replace(/&lt;/g, '<')
      .replace(/&gt;/g, '>')
      .replace(/&quot;/g, '"')
      .replace(/&#x27;/g, "'")
      .trim();

    if (content) parts.push(content);
  }

  if (parts.length > 0) {
    return parts.join('\n');
  }

  // Tenta padrão 2: Lyricks__Container (formato antigo do Genius)
  const oldRegex = /<div[^>]*class="[^"]*Lyrics__Container[^"]*"[^>]*>(.*?)<\/div>/gs;
  let oldMatch;

  while ((oldMatch = oldRegex.exec(html)) !== null) {
    let content = oldMatch[1]
      .replace(/<br\s*\/?>/gi, '\n')
      .replace(/<[^>]+>/g, '')
      .replace(/&amp;/g, '&')
      .replace(/&lt;/g, '<')
      .replace(/&gt;/g, '>')
      .replace(/&quot;/g, '"')
      .replace(/&#x27;/g, "'")
      .trim();

    if (content) parts.push(content);
  }

  if (parts.length > 0) {
    return parts.join('\n');
  }

  return null;
}

/**
 * Busca letras do Genius para uma música.
 *
 * @param {string} trackName
 * @param {string} artistName
 * @returns {Promise<string[]|null>} Array de linhas de texto ou null
 */
export async function getGeniusLyrics(trackName, artistName) {
  const slug = `${toSlug(artistName)}-${toSlug(trackName)}-lyrics`;
  const url = `https://genius.com/${slug}`;

  try {
    const res = await fetch(url, {
      headers: {
        'User-Agent': GENIUS_USER_AGENT,
        'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8',
        'Accept-Language': 'en-US,en;q=0.5',
      },
      signal: AbortSignal.timeout(8000),
    });

    if (!res.ok) return null;

    const html = await res.text();
    const lyricsText = extractLyricsFromHtml(html);
    if (!lyricsText) return null;

    // Divide em linhas, remove linhas vazias (exceto as que separam estrofes)
    const lines = lyricsText
      .split('\n')
      .map((l) => l.trim())
      .filter((l) => l.length > 0);

    if (lines.length < 3) return null; // muito curto, provavelmente não é letra

    return lines;
  } catch {
    return null;
  }
}

/**
 * Distribui linhas de letras não-sincronizadas uniformemente pela duração da música.
 * Cada linha recebe um timestamp estimado baseado em sua posição na letra.
 *
 * @param {string[]} lines - Array de linhas de texto
 * @param {number} durationMs - Duração total da música em ms
 * @returns {Array<{timeMs: number, text: string}>}
 */
export function distributeUnsyncedLines(lines, durationMs) {
  if (!lines || lines.length === 0) return [];

  const usableDuration = Math.max(durationMs || 240_000, 30_000); // min 30s
  const interval = usableDuration / lines.length;

  return lines.map((text, i) => ({
    timeMs: Math.round(i * interval),
    text,
  }));
}
