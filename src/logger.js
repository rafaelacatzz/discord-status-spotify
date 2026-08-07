import * as os from 'os';

const isInteractive = process.stdout.isTTY === true;

const LOG_LEVELS = { error: 0, warn: 1, info: 2, debug: 3 };

function getLevel() {
  const raw = (process.env.LOG_LEVEL || 'info').toLowerCase().trim();
  return Object.prototype.hasOwnProperty.call(LOG_LEVELS, raw) ? raw : 'info';
}

function getBool(name, def) {
  const raw = process.env[name];
  if (raw === undefined) return def;
  return ['1', 'true', 'yes', 'on'].includes(String(raw).toLowerCase());
}

const level = getLevel();
const minLevel = LOG_LEVELS[level];

// Compact reduz spam: sem debug e sem linhas repetitivas.
const compact = getBool('LOG_COMPACT', true);
const spinner = getBool('LOG_SPINNER', isInteractive);

const ansi = {
  reset: isInteractive ? '\x1b[0m' : '',
  bright: isInteractive ? '\x1b[1m' : '',
  dim: isInteractive ? '\x1b[2m' : '',
  red: isInteractive ? '\x1b[31m' : '',
  green: isInteractive ? '\x1b[32m' : '',
  yellow: isInteractive ? '\x1b[33m' : '',
  cyan: isInteractive ? '\x1b[36m' : '',
  magenta: isInteractive ? '\x1b[35m' : '',
  white: isInteractive ? '\x1b[37m' : '',
  gray: isInteractive ? '\x1b[90m' : '',
};

function c(color) {
  return ansi[color] || '';
}

function ts() {
  const d = new Date();
  return d.toISOString().replace('T', ' ').replace('Z', '');
}

function shouldLog(lvl) {
  return LOG_LEVELS[lvl] <= minLevel;
}

function tagColor(tag) {
  const key = String(tag).toUpperCase();
  if (key.includes('ERROR') || key.includes('STATUS')) return 'red';
  if (key.includes('WARN')) return 'yellow';
  if (key.includes('LYRICS') || key.includes('APP')) return 'cyan';
  if (key.includes('SPOTIFY')) return 'magenta';
  return 'white';
}

let lastLine = '';
let spinnerIndex = 0;
const SPINNER = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

function renderLine(line) {
  // overwrite para reduzir spam quando rodando em TTY
  if (!isInteractive || !spinner || !compact) {
    process.stdout.write(line + os.EOL);
    lastLine = line;
    return;
  }

  // TTY overwrite (mesma linha)
  if (line !== lastLine) {
    process.stdout.write(`\r${line}`);
    lastLine = line;
  }
}

function format(levelName, tag, message, meta) {
  const tagStr = tag
    ? `${c(tagColor(tag))}${ansi.bright}${tag.toUpperCase()}${ansi.reset}`
    : '';

  const base = `${c('gray')}${ts()}${ansi.reset} ${levelName.toUpperCase()}`;
  const sep = tagStr ? ' ' : '';
  const metaStr = meta !== undefined ? ` ${c('dim')}${JSON.stringify(meta)}${ansi.reset}` : '';
  return `${base}${sep}${tagStr} ${message}${metaStr}`;
}

export function log(levelName, tag, message, meta) {
  if (!shouldLog(levelName)) return;

  // Em modo compacto, info/debug podem virar overwrite ou serem suprimidos.
  if (compact && (levelName === 'debug' || (levelName === 'info' && tag && tag.toUpperCase().includes('LINE')))) {
    // suprime debug e eventos de linha
    return;
  }

  const line = format(levelName, tag, message, meta);

  // logs de erro/alerta vão para stderr para não misturar
  if (levelName === 'error' || levelName === 'warn') {
    process.stderr.write(line + os.EOL);
  } else {
    renderLine(line);
  }
}

export const logger = {
  error: (tag, message, meta) => log('error', tag, message, meta),
  warn: (tag, message, meta) => log('warn', tag, message, meta),
  info: (tag, message, meta) => log('info', tag, message, meta),
  debug: (tag, message, meta) => log('debug', tag, message, meta),
};

export function setCompact(next) {
  // simples utilitário caso alguém queira alterar em runtime
  // (não usado hoje, mas mantido por compatibilidade futura)
}

