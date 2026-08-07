import { logger } from './logger.js';

const isInteractive = process.stdout.isTTY === true;

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

const c = (color) => ansi[color] || '';

const DIVIDER = '─';
const WIDTH = 52;

function hr() {
  return DIVIDER.repeat(WIDTH);
}

function pad(text, width = WIDTH) {
  return text + ' '.repeat(Math.max(0, width - text.length));
}

export function banner() {
  logger.info('APP', 'Starting Discord Lyrics Status v1.0.0');
  console.log();
  console.log(`${c('cyan')}${hr()}${c('reset')}`);
  console.log(
    `${c('cyan')}│${c('reset')} ${c('bright')}${c('white')}Discord Lyrics Status${c('reset')} ${c('dim')}${c('gray')}v1.0.0${c('reset')} ${c('cyan')}│${c('reset')}`
  );
  console.log(`${c('cyan')}${hr()}${c('reset')}`);
  console.log();
}

export function statusBar(tokenOk, platform, trackName, artistName) {
  const token = tokenOk
    ? `${c('green')}● Connected${c('reset')}`
    : `${c('red')}● Not configured${c('reset')}`;

  let trackInfo = '';
  if (trackName && artistName) {
    trackInfo = `${c('bright')}${trackName}${c('reset')} — ${c('white')}${artistName}${c('reset')}`;
  } else if (trackName) {
    trackInfo = `${c('bright')}${trackName}${c('reset')}`;
  }

  const platformStr = platform ? `[${c('cyan')}${platform.toUpperCase()}${c('reset')}]` : '';

  console.log(`${c('cyan')}${hr()}${c('reset')}`);
  console.log(`${c('cyan')}│${c('reset')} Token: ${token}  ${platformStr}`);
  if (trackInfo) {
    console.log(`${c('cyan')}│${c('reset')} ${c('dim')}▶${c('reset')} ${trackInfo}`);
  }
  console.log(`${c('cyan')}${hr()}${c('reset')}`);
  console.log();
}

export function starting() {
  logger.info('APP', 'Starting...');
  console.log(`${c('cyan')}▶${c('reset')} Starting...`);
}

export function shuttingDown() {
  logger.info('APP', 'Shutting down...');
  console.log(`${c('cyan')}■${c('reset')} Shutting down...`);
}

export function trackChange(trackName, artistName, platform) {
  console.log(
    `${c('cyan')}▶${c('reset')} Now playing: ${c('bright')}${trackName}${c('reset')} — ${c('white')}${artistName}${c('reset')}` +
      (platform ? ` ${c('dim')}(${platform.toUpperCase()})${c('reset')}` : '')
  );
}

export function playbackStopped() {
  logger.warn('APP', 'Playback stopped. Status cleared.');
  console.log(`${c('yellow')}■${c('reset')} Playback stopped. Status cleared.`);
}

export function seekDetected(driftSec) {
  logger.info('APP', `Seek detected (${driftSec.toFixed(1)}s). Resyncing...`);
  console.log(`${c('yellow')}↻${c('reset')} Seek detected (${driftSec.toFixed(1)}s). Resyncing...`);
}

export function pollError(message) {
  logger.error('APP', message);
  console.error(`${c('red')}✗${c('reset')} ${message}`);
}

export function lyricsInfo(trackName, count) {
  logger.info('LYRICS', `Loaded ${count} synced lines for ${trackName}`);
  console.log(`${c('green')}♪${c('reset')} Loaded ${count} synced lines for ${c('white')}${trackName}${c('reset')}`);
}

export function lyricsNotFound(trackName, artistName) {
  logger.warn('LYRICS', `No lyrics found for ${trackName} — ${artistName}`);
  console.log(`${c('yellow')}!${c('reset')} No lyrics found for ${c('white')}${trackName}${c('reset')}`);
}

export function noSyncedLyrics(trackName) {
  logger.warn('LYRICS', `No synced lyrics for ${trackName} — using track name.`);
  console.log(`${c('yellow')}!${c('reset')} No synced lyrics for ${c('white')}${trackName}${c('reset')} — using track name.`);
}

export function lyricLine(text) {
  logger.debug('LINE', 'Lyric line', { text });
  console.log(`  ${c('magenta')}♫${c('reset')} ${c('bright')}${text}${c('reset')}`);
}

export function statusUpdated(text) {
  logger.debug('STATUS', 'Status updated', { text });
  console.log(`${c('green')}✓${c('reset')} Status updated`);
}

export function statusCleared() {
  logger.info('STATUS', 'Status cleared');
  console.log(`${c('yellow')}✓${c('reset')} Status cleared`);
}

export function rateLimited(waitMs) {
  logger.warn('STATUS', `Rate limited — retrying in ${waitMs}ms...`);
  console.warn(`${c('yellow')}⚠${c('reset')} Rate limited — retrying in ${waitMs}ms...`);
}

export function statusError(status, text) {
  logger.error('STATUS', `Error ${status}`, { text });
  console.error(`${c('red')}✗${c('reset')} Error ${status}: ${text}`);
}

export function fetchError(source, message) {
  logger.error('FETCH', `${source}`, { message });
  console.error(`${c('red')}✗${c('reset')} ${source}: ${message}`);
}

export function configTokenSaved(path) {
  logger.info('CONFIG', 'Token saved', { path });
  console.log(`${c('blue')}⚙${c('reset')} Token saved`);
}

export function setupOpening(url) {
  logger.info('SETUP', 'Opening browser...', { url });
  console.log(`${c('blue')}⚙${c('reset')} Opening browser...`);
}

export function setupValid(user) {
  logger.info('SETUP', `Logged in as ${user}`);
  console.log(`${c('green')}✓${c('reset')} Logged in as ${c('bright')}${user}${c('reset')}`);
}

export function setupError(message) {
  logger.error('SETUP', message);
  console.log(`${c('red')}✗${c('reset')} ${message}`);
}

export function setupNoToken() {
  logger.warn('SETUP', 'No token found. Starting setup...');
  console.log(`${c('blue')}⚙${c('reset')} No token found. Starting setup...`);
}

export function setupDone() {
  logger.info('SETUP', 'Setup complete. Starting monitor.');
  console.log(`${c('green')}✓${c('reset')} Setup complete. Starting monitor.\n`);
}

export function schedulerRestart(timeSec) {
  console.log(`${c('yellow')}↻${c('reset')} Scheduler restarted at ${timeSec}s`);
}

export function schedulerWaiting() {
  console.log(`${c('dim')}⏳ Waiting for lyrics...${c('reset')}`);
}

export function smtcError(message) {
  console.error(`${c('red')}✗${c('reset')} SMTC: ${message}`);
}

export function albumArtFound(trackName) {
  logger.info('ALBUMART', `Cover art found for ${trackName}`);
  console.log(`${c('green')}🖼${c('reset')} Cover art found for ${c('white')}${trackName}${c('reset')}`);
}

export function tokenOk() {
  console.log(`${c('green')}✓${c('reset')} Token loaded. Starting monitor.\n`);
}

export function idleBackoff(intervalMs) {
  logger.info('APP', `No music for a while — idle polling slowed to ${intervalMs / 1000}s`);
  console.log(`${c('dim')}⏳${c('reset')} No music for a while — idle polling slowed to ${intervalMs / 1000}s`);
}

export function idleBackoffResumed() {
  logger.info('APP', 'Playback detected — polling back to fast');
  console.log(`${c('green')}▶${c('reset')} Playback detected — polling back to fast`);
}

