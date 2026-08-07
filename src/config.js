import { readFileSync, writeFileSync, mkdirSync, existsSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';
import * as ui from './ui.js';

const CONFIG_DIR = join(
  process.env.APPDATA || join(homedir(), '.config'),
  'discord-lyrics-status'
);
const CONFIG_FILE = join(CONFIG_DIR, 'config.json');

let _cache = null;

function read() {
  if (_cache) return _cache;
  try {
    if (existsSync(CONFIG_FILE)) {
      _cache = JSON.parse(readFileSync(CONFIG_FILE, 'utf8'));
      return _cache;
    }
  } catch {}
  return {};
}

/**
 * Retorna o token de usuário do Discord.
 * Prioridade: variável de ambiente → config do AppData.
 */
export function getToken() {
  if (process.env.DISCORD_USER_TOKEN) return process.env.DISCORD_USER_TOKEN;
  return read().userToken || null;
}

/**
 * Persiste um novo token no AppData.
 */
export function saveToken(token) {
  _cache = null;
  mkdirSync(CONFIG_DIR, { recursive: true });
  const updated = { ...read(), userToken: token };
  _cache = updated;
  writeFileSync(CONFIG_FILE, JSON.stringify(updated, null, 2), 'utf8');
  ui.configTokenSaved(CONFIG_FILE);
}

/**
 * Lê uma preferência persistida (ecoMode, etc.).
 * @param {string} key
 * @param {*} fallback
 */
export function getSetting(key, fallback = null) {
  const v = read()[key];
  return v === undefined ? fallback : v;
}

/**
 * Persiste uma preferência (ecoMode, etc.) no mesmo config.json.
 * @param {string} key
 * @param {*} value
 */
export function saveSetting(key, value) {
  // Invalida o cache antes de ler (igual ao saveToken) para mesclar
  // alterações externas do config.json, se houver.
  _cache = null;
  mkdirSync(CONFIG_DIR, { recursive: true });
  const updated = { ...read(), [key]: value };
  _cache = updated;
  writeFileSync(CONFIG_FILE, JSON.stringify(updated, null, 2), 'utf8');
}
