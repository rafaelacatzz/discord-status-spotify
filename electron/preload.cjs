const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('electronAPI', {
  // Notificações nativas
  sendNotification: (title, body) => {
    ipcRenderer.send('show-notification', { title, body });
  },

  // Abrir links externos no navegador padrão
  openExternal: (url) => {
    ipcRenderer.send('open-external', url);
  },

  // Minimizar para o tray
  minimizeToTray: () => {
    ipcRenderer.send('minimize-to-tray');
  },

  // ── Fullscreen ───────────────────────────────────────────────────────────
  toggleFullscreen: () => {
    ipcRenderer.send('toggle-fullscreen');
  },

  onFullscreenChange: (callback) => {
    const handler = (_event, isFullscreen) => callback(isFullscreen);
    ipcRenderer.on('fullscreen-changed', handler);
    // Retorna função de cleanup
    return () => ipcRenderer.removeListener('fullscreen-changed', handler);
  },

  // Versão do app (seção Sobre)
  getAppVersion: () => {
    return ipcRenderer.invoke('get-app-version');
  },

  // ── Fechar para a bandeja vs. sair (Configurações → Desempenho) ──────
  setCloseToTray: (enabled) => {
    ipcRenderer.send('set-close-to-tray', enabled === true);
  },

  getCloseToTray: () => {
    return ipcRenderer.invoke('get-close-to-tray');
  },

  // Informações da plataforma
  platform: process.platform,
  versions: {
    node: process.versions.node,
    electron: process.versions.electron,
    chrome: process.versions.chrome,
  },
});
