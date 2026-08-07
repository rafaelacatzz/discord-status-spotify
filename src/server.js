import express from 'express';
import { createServer } from 'http';
import { WebSocketServer } from 'ws';
import { resolve, dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { existsSync, mkdirSync, writeFileSync } from 'fs';
import { homedir } from 'os';

// ── Resolve __dirname para funcionar tanto em ESM (dev) quanto CJS (build) ─
const _dirname = (() => {
  try {
    return dirname(fileURLToPath(import.meta.url));
  } catch {
    return typeof __dirname !== 'undefined' ? __dirname : process.cwd();
  }
})();

// Tenta múltiplos caminhos para o diretório public/:
function resolvePublicDir() {
  const candidates = [
    // Desenvolvimento (source)
    resolve(_dirname, '..', 'public'),              // src/../public
    // Bundle (dist/backend.cjs → dist/public/)
    resolve(_dirname, 'public'),                    // BUNDLE: dist/public  ← ESSENCIAL!
    // CWD variations
    resolve(process.cwd(), 'public'),               // node rodando de dist/
    resolve(process.cwd(), '..', 'public'),         // node rodando da raiz
    // Electron portable build (resources/app/)
    resolve(_dirname, '..', '..', '..', 'public'),   // resources/app/public
    resolve(_dirname, '..', '..', '..', '..', 'public'), // resources/public
    resolve(_dirname, '..', 'public'),               // resources/app/dist/public
    // UtilityProcess: __dirname pode ser resources/app/dist/
    resolve(_dirname, '..', '..', 'public'),         // resources/dist/../public → resources/public
    resolve(_dirname, '..', '..', '..', '..', '..', 'public'), // mais um nível acima
    // Fallback: junto do executável
    resolve(dirname(process.execPath), 'public'),
    resolve(dirname(process.execPath), '..', 'public'),
    // Absolute last resort
    resolve(_dirname, '..', '..', '..', '..', '..', '..', 'public'),
  ];
  for (const p of candidates) {
    try {
      if (existsSync(p)) return p;
    } catch {}
  }
  return resolve(process.cwd(), 'public');
}

const PUBLIC_DIR = resolvePublicDir();

// Tenta uma porta fixa, caindo para aleatória se ocupada
const PREFERRED_PORT = 18542;

/**
 * Gerenciador do servidor HTTP + WebSocket.
 * Serve o dashboard web, API REST e eventos em tempo real via WebSocket.
 */
export class DashboardServer {
  constructor() {
    this.app = express();
    this.server = createServer(this.app);
    /** @type {Set<import('ws').WebSocket>} */
    this.wsClients = new Set();
    this.port = null;
    this.url = null;

    this.#setupWebSocket();
    this.#setupMiddleware();
    this.#setupRoutes();
  }

  // ── WebSocket ──────────────────────────────────────────────────────────────
  #setupWebSocket() {
    // NOTA: `pingInterval`/`pingTimeout` NÃO são opções do WebSocketServer do
    // ws — eram silenciosamente ignoradas, então o keep-alive nunca rodava e
    // conexões ociosas eram derrubadas por proxies/firewalls. O heartbeat real
    // precisa ser manual: ping periódico + verificação de isAlive.
    this.wss = new WebSocketServer({
      server: this.server,
      path: '/api/ws',
      maxPayload: 1024 * 1024, // 1 MB max payload
    });

    this.wss.on('connection', (ws) => {
      ws.isAlive = true;
      ws.on('pong', () => { ws.isAlive = true; });

      this.wsClients.add(ws);

      // Envia estado inicial assim que conectar
      try {
        ws.send(JSON.stringify({
          type: 'state',
          data: this.#collectState(),
        }));
      } catch {}

      ws.on('close', () => {
        this.wsClients.delete(ws);
      });

      ws.on('error', () => {
        this.wsClients.delete(ws);
      });
    });

    // ── Heartbeat real (keep-alive) ───────────────────────────────────────
    // A cada 25s, envia ping para todos os clientes. Quem não respondeu ao
    // ping ANTERIOR (isAlive === false) é considerado morto e é encerrado,
    // evitando acúmulo de conexões zumbis (memória) e quedas por inatividade.
    this._heartbeatTimer = setInterval(() => {
      for (const client of this.wss.clients) {
        if (client.isAlive === false) {
          // terminate() dispara o evento 'close' do lado do servidor, que já
          // remove o cliente de wsClients — sem delete manual redundante.
          try { client.terminate(); } catch {}
          continue;
        }
        client.isAlive = false;
        try { client.ping(); } catch {}
      }
    }, 25000);
    this._heartbeatTimer.unref?.();
  }

  // ── Inicialização ─────────────────────────────────────────────────────────
  #setupMiddleware() {
    this.app.use(express.json());
    this.app.use(express.static(PUBLIC_DIR, {
      maxAge: 0,
      etag: false,
    }));
  }

  #setupRoutes() {
    // ── API: Status atual ────────────────────────────────────────────────
    this.app.get('/api/status', (_req, res) => {
      res.json(this.#collectState());
    });

    // ── API: Seek (clique na barra de progresso) ────────────────────
    this.app.post('/api/seek', async (req, res) => {
      const { positionMs } = req.body || {};
      if (positionMs === undefined || positionMs < 0) {
        return res.status(400).json({ error: 'positionMs required' });
      }
      try {
        if (!this.#seekHandler) {
          return res.status(503).json({ error: 'Seek handler not available' });
        }
        await this.#seekHandler(Math.floor(positionMs));
        res.json({ ok: true });
      } catch (err) {
        console.error('[Seek] Error:', err.message);
        res.status(500).json({ error: err.message });
      }
    });

    // ── API: Configurações (sincronia, desempenho, etc.) ───────────────
    this.app.post('/api/settings', (req, res) => {
      const body = req.body || {};
      const patch = {};
      if (body.syncLeadMs !== undefined) {
        const lead = parseInt(body.syncLeadMs, 10);
        if (lead >= 0 && lead <= 500) patch.syncLeadMs = lead;
      }
      if (typeof body.ecoMode === 'boolean') {
        patch.ecoMode = body.ecoMode;
      }
      // Emotes personalizados do status (unicode ou <:name:id>/<a:name:id>)
      if (typeof body.trackEmoji === 'string') {
        patch.trackEmoji = body.trackEmoji.trim().slice(0, 200);
      }
      if (typeof body.lyricsEmoji === 'string') {
        patch.lyricsEmoji = body.lyricsEmoji.trim().slice(0, 200);
      }
      // Flags de teste/restauração dos emotes (aplicam sem persistir no config)
      if (body.testEmotes === true) patch.testEmotes = true;
      if (body.revertEmotes === true) patch.revertEmotes = true;
      // Reativar o adaptive sync (botão "Reativar ajuste automático" na UI)
      if (body.adaptiveSync === true) patch.adaptiveSync = true;
      if (Object.keys(patch).length > 0 && this.#onSettingsChange) {
        this.#onSettingsChange(patch);
      }
      res.json({ ok: true });
    });

    // ── Rota: Setup ─────────────────────────────────────────────────────
    this.app.get('/setup', (_req, res) => {
      const setupPath = resolve(PUBLIC_DIR, 'setup.html');
      if (existsSync(setupPath)) {
        res.sendFile(setupPath);
      } else {
        res.redirect('/?setup=1');
      }
    });
  }

  /**
   * Registra o handler 404 após rotas externas serem montadas.
   */
  registerNotFoundHandler() {
    this.app.use((_req, res) => {
      res.status(404).json({ error: 'Not found' });
    });
  }

  // ── Coleta de estado (substituído externamente) ─────────────────────────
  /** @type {() => object} */
  #stateCollector = () => ({});

  /** @type {((settings: object) => void) | null} */
  #onSettingsChange = null;

  /** @type {((positionMs: number) => Promise<void>) | null} */
  #seekHandler = null;

  /**
   * Registra callback para quando o frontend alterar configurações.
   * @param {(settings: object) => void} fn
   */
  onSettingsChange(fn) {
    this.#onSettingsChange = fn;
  }

  /**
   * Registra callback para seek (mover posição da faixa via SMTC).
   * @param {(positionMs: number) => Promise<void>} fn
   */
  setSeekHandler(fn) {
    this.#seekHandler = fn;
  }

  /**
   * true se houver pelo menos um cliente WebSocket conectado ao dashboard.
   * Permite ao backend pular trabalho caro (coleta/serialização de estado)
   * quando ninguém está assistindo — economia real de CPU no modo ocioso.
   * @returns {boolean}
   */
  hasClients() {
    return this.wsClients.size > 0;
  }

  setStateCollector(fn) {
    this.#stateCollector = fn;
  }

  #collectState() {
    return this.#stateCollector();
  }

  /**
   * Versão pública de #collectState — usada pelo index.js para throttle de broadcasts.
   * @returns {object}
   */
  collectState() {
    return this.#collectState();
  }

  // ── Broadcast ────────────────────────────────────────────────────────────
  /**
   * Envia uma mensagem JSON para todos os clientes WebSocket conectados.
   *
   * Estrutura da mensagem: { type: <event>, data: <data> }
   *
   * Se `data` já for uma string JSON (pré-serializada pelo shouldBroadcast),
   * insere diretamente para evitar JSON.stringify duplicado.
   *
   * @param {string} event Nome do evento (type da mensagem)
   * @param {*} data Dados do evento (ou string JSON pré-serializada)
   */
  broadcast(event, data) {
    // Sem clientes conectados → nada a enviar, nem a serializar.
    if (this.wsClients.size === 0) return;

    let payload;
    if (typeof data === 'string') {
      // Otimização: data já é JSON string (pré-serializado pelo shouldBroadcast)
      payload = `{"type":"${event}","data":${data}}`;
    } else {
      payload = JSON.stringify({ type: event, data });
    }

    for (const client of this.wsClients) {
      try {
        client.send(payload);
      } catch {
        this.wsClients.delete(client);
      }
    }
  }

  /**
   * Envia estado completo para todos os clientes.
   * Sem clientes, nem coleta o estado (montar o objeto inclui o array de
   * letras — o mais caro) — o guard de broadcast() já evitaria a serialização,
   * mas evitar a coleta poupa o trabalho de construção do objeto também.
   */
  broadcastState() {
    if (this.wsClients.size === 0) return;
    this.broadcast('state', this.#collectState());
  }

  /**
   * Envia uma entrada de log para o dashboard.
   * @param {string} tag
   * @param {string} message
   */
  broadcastLog(tag, message) {
    this.broadcast('log', { tag, message });
  }

  // ── Iniciar servidor ─────────────────────────────────────────────────────
  /**
   * Tenta iniciar na porta preferida; se falhar, usa porta aleatória.
   * @returns {Promise<number>} Porta em que o servidor está ouvindo
   */
  start() {
    return new Promise((resolvePort) => {
      const tryPort = (port) => {
        this.server.removeAllListeners('error');
        this.server.once('error', (err) => {
          if (err.code === 'EADDRINUSE' && port === PREFERRED_PORT) {
            // Porta preferida ocupada — tenta aleatória
            tryPort(0);
          } else {
            console.error('[Server] Failed to start:', err.message);
            resolvePort(null);
          }
        });

        this.server.listen(port, '127.0.0.1', () => {
          const addr = this.server.address();
          this.port = addr.port;
          this.url = `http://127.0.0.1:${this.port}`;
          console.log(`[Server] Dashboard: ${this.url}`);
          console.log(`[Server] WebSocket: ws://127.0.0.1:${this.port}/api/ws`);

          // Persiste a URL em arquivo compartilhado para o Electron ler
          try {
            const urlDir = join(
              process.env.APPDATA || join(homedir(), '.config'),
              'discord-lyrics-status'
            );
            mkdirSync(urlDir, { recursive: true });
            writeFileSync(join(urlDir, 'server.url'), this.url, 'utf8');
          } catch {}

          resolvePort(this.port);
        });
      };

      tryPort(PREFERRED_PORT);
    });
  }

  /**
   * Para o servidor HTTP e WebSocket.
   */
  stop() {
    // Para o heartbeat antes de fechar as conexões
    if (this._heartbeatTimer) {
      clearInterval(this._heartbeatTimer);
      this._heartbeatTimer = null;
    }

    // Fecha todas as conexões WebSocket
    for (const client of this.wsClients) {
      try {
        client.close(1001, 'Server shutting down');
      } catch {}
    }
    this.wsClients.clear();

    // Fecha o WebSocket server
    try {
      this.wss.close();
    } catch {}

    return new Promise((resolve) => {
      this.server.close(() => resolve());
    });
  }
}
