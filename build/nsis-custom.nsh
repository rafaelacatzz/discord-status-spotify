;;
;; nsis-custom.nsh — Extensoes personalizadas do instalador NSIS
;;
;; Adiciona ao instalador:
;;   1. Pagina "Configuracoes Adicionais" (autostart + atalho desktop)
;;   2. Atalho no registro para iniciar automaticamente com Windows
;;
;; Adiciona ao desinstalador:
;;   3. MessageBox perguntando se deseja manter ou remover dados
;;   4. Limpeza condicional do diretorio de configuracao
;;
;; Compativel com electron-builder v25+
;; NOTA: manter este arquivo em ASCII puro (sem acentos/emoji) para evitar
;; mojibake na compilacao NSIS em cp1252.
;;

; ── Headers obrigatorios ────────────────────────────────────────────────
; Nota: electron-builder NAO inclui estes headers antes do custom .nsh.
; Precisamos inclui-los explicitamente para usar ${If}, ${NSD_CreateCheckBox}, etc.
; MUI2.nsh e necessario para a macro MUI_HEADER_TEXT (guarda de inclusao propria).
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "nsDialogs.nsh"

; ══════════════════════════════════════════════════════════════════════
; INSTALADOR
; ══════════════════════════════════════════════════════════════════════

; ── Registra a pagina customizada no fluxo do instalador ───────────────
Page custom CustomSettingsPage CustomSettingsPageLeave

; ── Variaveis globais (declaradas antes de qualquer uso) ───────────────
Var CUSTOM_PAGE_AUTOSTART
Var CUSTOM_PAGE_DESKTOP_SHORTCUT

; ── Funcao: configura inicializacao automatica ─────────────────────────
Function SetAutoStart
  Pop $0  ; 1 = sim, 0 = nao

  ${If} $0 == 1
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" \
              "Discord Lyrics Status" \
              "$INSTDIR\Discord Lyrics Status.exe"
  ${Else}
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" \
                  "Discord Lyrics Status"
  ${EndIf}
FunctionEnd

; ── Pagina personalizada: Configuracoes Adicionais ─────────────────────
Function CustomSettingsPage
  !insertmacro MUI_HEADER_TEXT "Configuracoes Adicionais" \
                                "Personalize a instalacao do Discord Lyrics Status"

  nsDialogs::Create 1018
  Pop $0

  ${If} $0 == error
    Abort
  ${EndIf}

  ; Checkbox 1: iniciar com Windows
  ${NSD_CreateCheckBox} 0u 10u 100% 12u "Iniciar o Discord Lyrics Status automaticamente quando o Windows iniciar"
  Pop $CUSTOM_PAGE_AUTOSTART

  ; Relembra escolha anterior (se reinstalando)
  ReadRegStr $1 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Discord Lyrics Status"
  ${If} $1 != ""
    ${NSD_Check} $CUSTOM_PAGE_AUTOSTART
  ${EndIf}

  ; Checkbox 2: atalho na area de trabalho
  ${NSD_CreateCheckBox} 0u 28u 100% 12u "Criar atalho na Area de Trabalho"
  Pop $CUSTOM_PAGE_DESKTOP_SHORTCUT

  ${If} ${FileExists} "$DESKTOP\Discord Lyrics Status.lnk"
    ${NSD_Check} $CUSTOM_PAGE_DESKTOP_SHORTCUT
  ${EndIf}

  ; Texto informativo
  ${NSD_CreateLabel} 0u 46u 100% 30u \
    "O Discord Lyrics Status sera executado em segundo plano e aparecera na bandeja do sistema.$\r$\n\
     Voce pode alterar estas configuracoes a qualquer momento dentro do aplicativo."

  nsDialogs::Show
FunctionEnd

Function CustomSettingsPageLeave
  ; Salva a escolha de autostart
  ${NSD_GetState} $CUSTOM_PAGE_AUTOSTART $0
  Push $0
  Call SetAutoStart

  ; Cria atalho na area de trabalho se marcado e nao existir
  ${NSD_GetState} $CUSTOM_PAGE_DESKTOP_SHORTCUT $0
  ${If} $0 == 1
    ${IfNot} ${FileExists} "$DESKTOP\Discord Lyrics Status.lnk"
      CreateShortCut "$DESKTOP\Discord Lyrics Status.lnk" \
                     "$INSTDIR\Discord Lyrics Status.exe" \
                     "" "$INSTDIR\Discord Lyrics Status.exe" 0
    ${EndIf}
  ${EndIf}
FunctionEnd

; ══════════════════════════════════════════════════════════════════════
; DESINSTALADOR
; ══════════════════════════════════════════════════════════════════════

; ── Caminho do diretorio de configuracao do usuario ─────────────────────
; Deve corresponder ao CONFIG_DIR em src/config.js
;   process.env.APPDATA || join(homedir(), '.config'), 'discord-lyrics-status'
!define USER_CONFIG_DIR "$APPDATA\discord-lyrics-status"

; ── Hook: inicio do desinstalador ──────────────────────────────────────
!macro customUnInit
  ; Remove entrada de inicializacao automatica do registro
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Discord Lyrics Status"

  ; Remove chave de configuracao do app no registro
  DeleteRegKey HKCU "Software\DiscordLyricsStatus"

  ; Tenta parar o app se estiver rodando (libera locks em arquivos de config)
  nsExec::ExecToStack 'taskkill /F /IM "Discord Lyrics Status.exe" 2>nul'
!macroend

; ── Hook: durante a desinstalacao — pergunta sobre dados do usuario ────
!macro customUnInstall
  ; So pergunta se existirem dados de configuracao
  ${If} ${FileExists} "${USER_CONFIG_DIR}\*.*"
    MessageBox MB_YESNO|MB_ICONQUESTION \
      "Remover tambem os dados de configuracao?$\r$\n$\r$\n\
       Seu token do Discord, cache e preferencias serao perdidos.$\r$\n$\r$\n\
       Escolha [Sim] para remover tudo.$\r$\n$\r$\n\
       Escolha [Nao] para preservar (recomendado se for reinstalar)." \
      IDNO skip_data_removal

      ; Usuario escolheu REMOVER
      RmDir /r "${USER_CONFIG_DIR}"
      DetailPrint "Dados de configuracao removidos."
      Goto data_done

    skip_data_removal:
      ; Usuario escolheu MANTER
      DetailPrint "Dados de configuracao preservados em: ${USER_CONFIG_DIR}"
      DetailPrint "Caso queira remover manualmente, delete a pasta acima ou execute o instalador novamente."
    data_done:
  ${Else}
    DetailPrint "Nenhum dado de configuracao encontrado para remover."
  ${EndIf}
!macroend
