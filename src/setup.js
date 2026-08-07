import { saveToken } from './config.js';
import * as ui from './ui.js';

function getDiscordAvatarUrl(user) {
  if (!user) return null;
  const id = user.id;
  const avatar = user.avatar;
  if (id && avatar) {
    const ext = avatar.startsWith('a_') ? 'gif' : 'png';
    return `https://cdn.discordapp.com/avatars/${id}/${avatar}.${ext}?size=128`;
  }
  if (typeof user.discriminator === 'string') {
    const index = Number(user.discriminator) % 5;
    return `https://cdn.discordapp.com/embed/avatars/${index}.png`;
  }
  return null;
}

function getDiscordTag(user) {
  if (!user) return null;
  if (user.username && typeof user.discriminator === 'string') {
    return `${user.username}#${user.discriminator}`;
  }
  return user.username || null;
}

export async function resolveDiscordAccount(token) {
  if (!token || typeof token !== 'string') return null;

  try {
    const res = await fetch('https://discord.com/api/v9/users/@me', {
      headers: { Authorization: token },
    });
    if (!res.ok) return null;

    const user = await res.json();
    return {
      username: user.username || null,
      discriminator: typeof user.discriminator === 'string' ? user.discriminator : null,
      tag: getDiscordTag(user),
      avatarUrl: getDiscordAvatarUrl(user),
    };
  } catch {
    return null;
  }
}

/**
 * Valida o token com a API do Discord e, se válido, persiste no disco.
 * @param {string} token
 * @returns {Promise<{ok: boolean, username?: string, discriminator?: string, tag?: string, avatarUrl?: string, error?: string}>}
 */
export async function validateAndSaveToken(token) {
  if (!token || typeof token !== 'string' || token.length < 20) {
    return { ok: false, error: 'Token muito curto ou inválido.' };
  }

  try {
    const check = await fetch('https://discord.com/api/v9/users/@me', {
      headers: { Authorization: token },
    });

    if (check.status === 401) {
      return { ok: false, error: 'Token inválido (Discord rejeitou). Verifique novamente.' };
    }
    if (!check.ok) {
      return { ok: false, error: `Discord retornou erro ${check.status}. Tente novamente mais tarde.` };
    }

    const user = await check.json();
    const account = {
      username: user.username || null,
      discriminator: typeof user.discriminator === 'string' ? user.discriminator : null,
      tag: getDiscordTag(user),
      avatarUrl: getDiscordAvatarUrl(user),
    };
    saveToken(token);
    ui.setupValid(account.username);
    return { ok: true, ...account };
  } catch {
    return { ok: false, error: 'Não foi possível conectar ao Discord. Verifique sua conexão.' };
  }
}

/**
 * Função de setup mantida para compatibilidade — retorna o HTML e inicia validação.
 * @param {import('./server.js').DashboardServer} server
 * @param {(account: {username: string|null, discriminator: string|null, tag: string|null, avatarUrl: string|null}) => void} [onTokenSaved]
 */
export function attachSetupRoutes(server, onTokenSaved = () => {}) {
  // Endpoint de validação (usado exclusivamente pelo overlay do dashboard)
  server.app.post('/api/setup', async (req, res) => {
    const result = await validateAndSaveToken(req.body?.token || '');
    res.json(result);

    if (result.ok) {
      onTokenSaved({
        username: result.username || null,
        discriminator: result.discriminator || null,
        tag: result.tag || null,
        avatarUrl: result.avatarUrl || null,
      });
      server.broadcastLog('SETUP', `Logged in as ${result.username}`);
      server.broadcast('status-changed', { status: 'setup-complete', user: result.username });

      // Aguarda um momento para o navegador mostrar o sucesso, depois notifica
      setTimeout(() => {
        server.broadcast('status-changed', { status: 'ready' });
      }, 2000);
    }
  });
}
