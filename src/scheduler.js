/**
 * LyricScheduler — polling-based scheduler for synced lyrics.
 *
 * Instead of per-line setTimeout (which accumulates drift), this uses a single
 * setInterval that checks which line should be active every 50ms.
 * The SMTC poll loop calls updateProgress() every ~1000ms to recalibrate the
 * internal clock against actual playback position — a closed-loop approach.
 */
import * as ui from './ui.js';

export class LyricScheduler {
  /** @type {Array<{timeMs: number, text: string}>} */
  #lyrics = [];

  /** @type {(line: {timeMs: number, text: string}) => void} */
  #onLineChange;

  /** @type {number} SMTC progressMs when start() was last called/recalibrated */
  #startProgressMs = 0;

  /** @type {number} performance.now() when #startProgressMs was set */
  #startedAt = 0;

  /** @type {boolean} Se o agendador está ativo */
  #active = false;

  /** @type {number} Última linha disparada (índice) */
  #lastFiredIndex = -1;

  /** @type {number|null} ID do intervalo de polling */
  #tickTimer = null;

  /** @type {number} Intervalo em ms entre ticks de verificação */
  #tickIntervalMs = 50;

  /** @type {number} Antecipação em ms para compensar latência da API do Discord */
  #leadMs = 200;

  /**
   * @param {Array<{timeMs: number, text: string}>} lyricsArray
   * @param {(line: {timeMs: number, text: string}) => void} onLineChange
   */
  constructor(lyricsArray, onLineChange) {
    this.#lyrics = lyricsArray;
    this.#onLineChange = onLineChange;
  }

  /**
   * Estima a posição atual de reprodução baseada no último progresso conhecido.
   * @returns {number}
   */
  #getCurrentProgress() {
    return this.#startProgressMs + (performance.now() - this.#startedAt);
  }

  /**
   * Dispara onLineChange para a linha que deve estar ativa.
   * Usa #leadMs de antecipação para compensar a latência da API do Discord.
   */
  #tick() {
    if (!this.#active) return;

    // Early exit: todas as letras já foram disparadas — evita binary search inútil
    if (this.#lastFiredIndex >= this.#lyrics.length - 1) return;

    // Aplica leadMs de antecipação: o scheduler age como se estivesse
    // um pouco à frente do progresso real, compensando o tempo do PATCH
    const currentProgress = this.#getCurrentProgress() + this.#leadMs;

    // Busca binária: encontra o índice da última linha com timeMs <= currentProgress
    // As letras já estão ordenadas por timeMs (garantido pelo parseLRC)
    let activeIndex = -1;
    let low = 0;
    let high = this.#lyrics.length - 1;

    while (low <= high) {
      const mid = (low + high) >>> 1;
      if (this.#lyrics[mid].timeMs <= currentProgress) {
        activeIndex = mid;
        low = mid + 1;
      } else {
        high = mid - 1;
      }
    }

    // Se encontrou uma nova linha para disparar
    if (activeIndex > this.#lastFiredIndex) {
      this.#lastFiredIndex = activeIndex;
      const line = this.#lyrics[activeIndex];
      try {
        this.#onLineChange({ ...line, index: activeIndex });
      } catch (err) {
        console.error('[Scheduler] Line error:', err.message);
      }
    }
  }

  /**
   * Inicia o agendamento a partir da posição de reprodução fornecida.
   * @param {number} progressMs - Posição atual de reprodução em ms
   */
  start(progressMs) {
    this.stop();

    this.#startedAt = performance.now();
    this.#startProgressMs = progressMs;
    this.#active = true;
    this.#lastFiredIndex = -1;

    // Dispara a linha atual imediatamente
    this.#tick();
    const firedImmediate = this.#lastFiredIndex >= 0;

    // Polling contínuo a cada 50ms para detectar mudanças de linha
    this.#tickTimer = setInterval(() => this.#tick(), this.#tickIntervalMs);

    if (!firedImmediate && this.#lyrics.length > 0) {
      ui.schedulerWaiting();
    }
  }

  /**
   * Calibra o relógio interno com a posição real do SMTC.
   * Chamado pelo poll loop (~1s) para fechar a malha de controle.
   *
   * IMPORTANTE: o SMTC reporta a posição quantizada em segundos inteiros.
   * Se o valor vier ATRASADO em relação ao relógio interno (que avança em
   * tempo real via performance.now), NÃO regredimos — senão cada poll faria
   * as letras dispararem atrasadas (o relógio "anda para trás" a cada 1s).
   * Só recalibramos quando o valor externo está À FRENTE (recuperou linhas)
   * ou atrasado além de um seek real (usuário voltou a faixa).
   * @param {number} actualProgressMs - Progresso real vindo do SMTC
   */
  updateProgress(actualProgressMs) {
    if (!this.#active) return;

    const internal = this.#getCurrentProgress();
    // Tolerância de quantização do SMTC (~1s). Acima disso = seek real para trás.
    const QUANTIZE_TOLERANCE_MS = 1200;
    const lag = internal - actualProgressMs;

    if (lag > QUANTIZE_TOLERANCE_MS) {
      // Seek real para trás: recalibra e permite re-disparar linhas anteriores.
      this.#startProgressMs = actualProgressMs;
      this.#startedAt = performance.now();
      this.#lastFiredIndex = -1;
    } else if (actualProgressMs > internal) {
      // SMTC à frente: recalibra para frente (recuperou linhas sem seek).
      this.#startProgressMs = actualProgressMs;
      this.#startedAt = performance.now();
    }
    // Senão (lag <= tolerância): mantém o relógio interno — mais preciso.

    // Verifica se há linhas para recuperar imediatamente
    this.#tick();
  }

  /**
   * Pausa o scheduler sem resetar o estado.
   * O progresso estimado é congelado até resume() ser chamado.
   * Útil quando a música pausa mas a faixa ainda está ativa (histerese).
   */
  pause() {
    if (!this.#active) return;
    this.#active = false;
    if (this.#tickTimer !== null) {
      clearInterval(this.#tickTimer);
      this.#tickTimer = null;
    }
    // Congela o progresso atual para que #getCurrentProgress() retorne
    // a posição no momento da pausa (não avança com performance.now())
    this.#startProgressMs = this.#getCurrentProgress();
    this.#startedAt = performance.now();
  }

  /**
   * Retoma o scheduler após pause().
   * O progresso continua do ponto onde parou (congelado por pause()).
   * @param {number} [actualProgressMs] - Opcional: posição real do SMTC para recalibrar
   */
  resume(actualProgressMs) {
    if (this.#active) return;

    if (actualProgressMs !== undefined) {
      this.#startProgressMs = actualProgressMs;
    }
    this.#startedAt = performance.now();
    this.#active = true;

    // Re-dispara a linha atual (pode ter mudado durante a pausa)
    this.#tick();

    // Re-inicia o polling
    this.#tickTimer = setInterval(() => this.#tick(), this.#tickIntervalMs);
  }

  /**
   * Para todos os timers pendentes.
   */
  stop() {
    this.#active = false;
    if (this.#tickTimer !== null) {
      clearInterval(this.#tickTimer);
      this.#tickTimer = null;
    }
  }

  /**
   * Ajusta o lead (antecipação) em tempo real.
   * Ex: 45ms significa que a letra aparece 45ms antes do tempo real.
   * @param {number} ms
   */
  setLeadMs(ms) {
    this.#leadMs = Math.max(0, Math.min(500, ms));
  }

  /**
   * Ajusta o intervalo de polling em tempo real (modo econômico usa um
   * intervalo maior para reduzir consumo de CPU). Reinicia o timer se ativo.
   * @param {number} ms
   */
  setTickIntervalMs(ms) {
    this.#tickIntervalMs = Math.max(30, ms);
    if (this.#active && this.#tickTimer !== null) {
      clearInterval(this.#tickTimer);
      this.#tickTimer = setInterval(() => this.#tick(), this.#tickIntervalMs);
    }
  }

  /**
   * Reinicia a partir de uma nova posição de reprodução (ex: após uma busca).
   * @param {number} newProgressMs
   */
  restart(newProgressMs) {
    ui.schedulerRestart((newProgressMs / 1000).toFixed(1));
    this.start(newProgressMs);
  }

  /**
   * Estima a posição atual de reprodução com base no último progresso conhecido.
   * @returns {number}
   */
  get estimatedProgressMs() {
    return this.#getCurrentProgress();
  }
}
