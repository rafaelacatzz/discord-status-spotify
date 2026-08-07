import { join } from 'path';
import { homedir } from 'os';
import { existsSync, unlinkSync, writeFileSync, readFileSync, mkdirSync, openSync, closeSync, constants } from 'fs';
import { execSync } from 'child_process';

const LOCK_FILE = join(
  process.env.APPDATA || join(homedir(), '.config'),
  'discord-lyrics-status',
  'instance.lock'
);

let locked = false;

/**
 * Tenta adquirir o lock de instância única.
 * Retorna true se CONSEGUIU o lock (é a única instância).
 * Retorna false se outra instância já está rodando.
 */
export function acquireLock() {
  if (locked) return true;

  try {
    const dir = join(LOCK_FILE, '..');
    mkdirSync(dir, { recursive: true });

    const pid = process.pid;
    const content = JSON.stringify({ pid, startedAt: Date.now() });

    let fd;
    try {
      fd = openSync(LOCK_FILE, constants.O_CREAT | constants.O_EXCL | constants.O_RDWR);
    } catch (err) {
      if (err.code === 'EEXIST') {
        if (isLockStale()) {
          // Lock expirado — remove e tenta novamente
          try { unlinkSync(LOCK_FILE); } catch {}
          try {
            fd = openSync(LOCK_FILE, constants.O_CREAT | constants.O_EXCL | constants.O_RDWR);
          } catch {
            return false;
          }
        } else {
          // Outra instância está rodando ativamente
          return false;
        }
      } else {
        // Erro inesperado — NEGA permissão (segurança > conveniência)
        return false;
      }
    }

    if (!fd) return false;

    try {
      writeFileSync(LOCK_FILE, content, 'utf8');
    } catch {
      try { closeSync(fd); } catch {}
      return false;
    }

    try {
      closeSync(fd);
    } catch {}

    locked = true;
    return true;
  } catch {
    // Em caso de erro inesperado, NEGA permissão para evitar múltiplas instâncias
    return false;
  }
}

export function releaseLock() {
  if (!locked) return;
  try {
    if (existsSync(LOCK_FILE)) {
      // Só remove se o lock ainda é nosso (verifica o PID)
      try {
        const data = JSON.parse(readFileSync(LOCK_FILE, 'utf8'));
        if (data && data.pid === process.pid) {
          unlinkSync(LOCK_FILE);
        }
      } catch {
        unlinkSync(LOCK_FILE);
      }
    }
  } catch {}
  locked = false;
}

function isLockStale() {
  try {
    const data = JSON.parse(readFileSync(LOCK_FILE, 'utf8'));
    if (!data || !data.pid) return true;

    // Se o PID é o mesmo que o atual, o lock é nosso (reentrância)
    if (data.pid === process.pid) return true;

    // Processo morto → lock stale
    if (!checkProcessAlive(data.pid)) return true;

    // PID vivo MAS de outro processo: no Windows os PIDs são reciclados
    // rapidamente após a morte do processo (kill forçado, crash). Se o dono
    // do PID não é o nosso app, a instância anterior morreu e o PID foi
    // reusado por outro programa → o lock é stale, podemos reclamá-lo.
    if (!checkProcessIsOurs(data.pid)) return true;

    // Processo vivo E é o nosso app → outra instância real está rodando
    return false;
  } catch {
    // Se não conseguiu ler, o lock está corrompido ou não existe
    return true;
  }
}

/**
 * Confirma que o processo dono do PID pertence ao NOSSO app (nome do exe e,
 * para node/electron, linha de comando). Protege contra PID reciclado pelo
 * Windows: um PID vivo que não é do nosso app não deve segurar o lock.
 */
function checkProcessIsOurs(pid) {
  try {
    const stdout = execSync(`tasklist /FI "PID eq ${pid}" /FO CSV /NH`, {
      encoding: 'utf8',
      timeout: 3000,
      windowsHide: true,
      // Sem stdio explícito, o stderr do filho (ex.: wmic imprimindo
      // "Nenhuma instância disponível." em pt-BR para PID morto) VAZA para
      // o stderr do backend e polui o log — e no arranque isso parecia um
      // erro do SMTC. Pipes explícitos capturam tudo.
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    // Formato CSV: "discord-lyrics-status.exe","1234","Console",...
    const m = stdout.match(/"([^"]+\.exe)"/i);
    const name = m ? m[1].toLowerCase() : '';
    if (!name) return false;

    // App empacotado / daemon SMTC: nome já é suficiente
    if (name === 'discord-lyrics-status.exe' || name === 'smtcdaemon.exe') return true;

    // node / electron (dev): só se a linha de comando aponta para o projeto
    if (name === 'node.exe' || name === 'electron.exe') {
      let cmdLine = '';
      try {
        cmdLine = execSync(`wmic process where "ProcessId=${pid}" get CommandLine /value`, {
          encoding: 'utf8',
          timeout: 3000,
          windowsHide: true,
          stdio: ['ignore', 'pipe', 'pipe'], // não vazar stderr do wmic
        });
      } catch {
        // wmic removido no Windows 11 22H2+ — fallback via PowerShell CIM
        try {
          cmdLine = execSync(
            `powershell -NoProfile -NonInteractive -Command "(Get-CimInstance Win32_Process -Filter 'ProcessId=${pid}').CommandLine"`,
            { encoding: 'utf8', timeout: 5000, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] }
          );
        } catch {}
      }
      return /discord-lyrics-status|discord-status-spotify|backend\.cjs|src[\\/]index\.js/i.test(cmdLine);
    }

    return false;
  } catch {
    return false;
  }
}

function checkProcessAlive(pid) {
  // Tentativa 1: sinal 0 (rápido, funciona para processos filhos)
  try {
    process.kill(pid, 0);
    return true;
  } catch {}

  // Tentativa 2: tasklist no Windows
  try {
    const stdout = execSync(`tasklist /FI "PID eq ${pid}" /NH`, {
      encoding: 'utf8',
      timeout: 3000,
      windowsHide: true,
      stdio: ['ignore', 'pipe', 'pipe'], // não vazar stderr do filho
    });
    if (stdout.includes(String(pid))) return true;
  } catch {}

  // Tentativa 3: wmic (fallback mais confiável em algumas situações)
  try {
    const stdout = execSync(`wmic process where "ProcessId=${pid}" get ProcessId`, {
      encoding: 'utf8',
      timeout: 3000,
      windowsHide: true,
      // Sem isto, o wmic imprime "Nenhuma instância disponível." (pt-BR)
      // no stderr quando o PID não existe e o ruído vaza para o log.
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    return stdout.includes(String(pid));
  } catch {}

  return false;
}
