using Windows.Media.Control;

namespace SmtcDaemon;

/// <summary>
/// Utilitário SMTC com dois modos de operação:
///
///   SmtcDaemon.exe                  → modo DAEMON (loop infinito, consulta a cada 1s)
///   SmtcDaemon.exe seek <positionMs> → modo SEEK (comando único, sai imediatamente)
///
/// COMPILAÇÃO (self-contained single-file + trimming para reduzir o
/// instalador de ~93 MB para ~13 MB):
///   npm run build:daemon
///   (ou: dotnet publish smtc-daemon/SmtcDaemon.csproj -c Release)
///
/// ⚠️  COM PublishTrimmed (TrimMode full) NÃO use APIs baseadas em reflexão
/// (ex.: System.Text.Json sem source generator — quebra em runtime com
/// "Reflection-based serialization has been disabled"). O JSON aqui é
/// montado manualmente de propósito. Valide a saída do daemon após editar.
/// </summary>
class Program
{
    static async Task<int> Main(string[] args)
    {
        Console.OutputEncoding = System.Text.Encoding.UTF8;

        if (args.Length > 0 && args[0].Equals("seek", StringComparison.OrdinalIgnoreCase))
        {
            // ── Modo SEEK: comando único ──────────────────────────────────
            if (args.Length < 2 || !long.TryParse(args[1], out long positionMs))
            {
                Console.Error.WriteLine("Usage: SmtcDaemon.exe seek <positionMs>");
                return 1;
            }

            return await SeekTrackAsync(positionMs) ? 0 : 1;
        }

        // ── Modo DAEMON: loop infinito (comportamento padrão) ─────────────
        await RunDaemonAsync();
        return 0; // never reached
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  DAEMON MODE
    // ═══════════════════════════════════════════════════════════════════════

    /// <summary>
    /// Loop infinito que consulta o SMTC a cada ~1s e escreve JSON no stdout.
    /// </summary>
    static async Task RunDaemonAsync()
    {
        // Watchdog anti-órfão: se o backend (processo pai, passado via
        // SMTC_PARENT_PID) morrer — taskkill /F, crash, kill -9 — o daemon se
        // auto-encerra em ≤1s em vez de ficar vivo monopolizando a sessão SMTC
        // e fazendo o próximo backend falhar com "Nenhuma instância disponível".
        // Sem SMTC_PARENT_PID (execução manual no terminal) o watchdog fica off.
        int? parentPid = null;
        if (int.TryParse(Environment.GetEnvironmentVariable("SMTC_PARENT_PID"), out int ppid))
            parentPid = ppid;

        // SELF-HEAL do manager stale: o RequestAsync às vezes retorna um manager
        // que não enxerga NENHUMA sessão (observado quando o daemon inicia logo
        // após outro daemon morrer/levar kill). Re-RequestAsync no MESMO processo
        // NÃO cura (o SessionManager fica preso por processo — validado: só um
        // processo novo enxerga a sessão). Por isso o daemon SAI do processo nos
        // primeiros ~15s se nenhuma sessão aparecer, e o backend reinicia
        // (processo novo = SessionManager novo).
        //
        // O backend desliga o self-heal (SMTC_NO_SELF_HEAL=1) após o primeiro
        // exit sem dados — assim o 2º processo fica no heartbeat de 2s quando o
        // Spotify está genuinamente fechado (o daemon não tem como distinguir
        // "manager stale" de "Spotify fechado"), evitando loop de restart.
        long startupGraceMs = Environment.TickCount64;
        int noSessionStreak = 0;
        bool allowSelfHeal = Environment.GetEnvironmentVariable("SMTC_NO_SELF_HEAL") != "1";

        while (true)
        {
            if (!IsParentAlive(parentPid))
            {
                Console.Error.WriteLine("[SMTC] Parent process exited — shutting down.");
                return;
            }

            try
            {
                var manager = await GlobalSystemMediaTransportControlsSessionManager.RequestAsync();

                while (true)
                {
                    // Re-checa o pai a cada ciclo do loop interno (1s tocando / 2s ocioso)
                    if (!IsParentAlive(parentPid))
                    {
                        Console.Error.WriteLine("[SMTC] Parent process exited — shutting down.");
                        return;
                    }

                    try
                    {
                        // SEMPRE reporta o Spotify, mesmo quando o sistema considera
                        // outra app (ex: YouTube no navegador) como "sessão atual" —
                        // isso elimina o flip-flop de fontes no backend.
                        var session = PickSpotifySession(manager);
                        if (session == null)
                        {
                            // Sem sessão do Spotify: mantém o heartbeat do backend
                            // ({isPlaying:false}) sem vazar dados de outras apps.
                            // Backoff de 2s: o backend tolera até 15s sem emissão
                            // (liveness), então 2s ocioso economiza ~50% do trabalho
                            // do daemon quando o Spotify está fechado. Assim que uma
                            // sessão aparecer, volta ao ritmo de 1s imediatamente.
                            //
                            // Manager stale no boot: se nas primeiras ~15s de vida
                            // nenhuma sessão aparecer, o manager provavelmente é um
                            // resíduo (RequestAsync mal sucedido) — sair faz o
                            // backend reiniciar com um processo novo (única cura real).
                            // Desligado via SMTC_NO_SELF_HEAL após o 1º exit sem dados.
                            if (allowSelfHeal
                                && Environment.TickCount64 - startupGraceMs < 15000
                                && ++noSessionStreak >= 3)
                            {
                                Console.Error.WriteLine("[SMTC] No sessions visible at startup — exiting for restart (stale SessionManager).");
                                return;
                            }
                            Console.WriteLine("""{"isPlaying":false}""");
                            await Task.Delay(2000);
                            continue;
                        }
                        noSessionStreak = 0;

                        var props = await session.TryGetMediaPropertiesAsync();
                        var pb = session.GetPlaybackInfo();
                        var tl = session.GetTimelineProperties();

                        var sourceApp = session.SourceAppUserModelId ?? "";
                        var platform = DetectPlatform(sourceApp);

                        // Quando tl é null (raro), usa UtcNow para não quebrar o progresso
                        // Nota: LastUpdatedTime já vem como DateTimeOffset na projeção WinRT
                        long lastUpdated = tl?.LastUpdatedTime is { } t
                            ? t.ToUnixTimeMilliseconds()
                            : DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();

                        long positionMs = (long)(tl?.Position.TotalMilliseconds ?? 0);
                        long durationMs = (long)(tl?.EndTime.TotalMilliseconds ?? 0);

                        // JSON manual (sem System.Text.Json): com PublishTrimmed
                        // o serializer por reflexão é desabilitado e o tipo anônimo
                        // falhava em runtime. EscapeJson escapa strings; números e
                        // booleans são sempre seguros. Mesmo formato de antes.
                        var isPlaying = pb?.PlaybackStatus
                            == GlobalSystemMediaTransportControlsSessionPlaybackStatus.Playing;
                        Console.WriteLine(
                            "{\"title\":\"" + EscapeJson(props?.Title ?? "") +
                            "\",\"artist\":\"" + EscapeJson(props?.Artist ?? "") +
                            "\",\"albumTitle\":\"" + EscapeJson(props?.AlbumTitle ?? "") +
                            "\",\"isPlaying\":" + (isPlaying ? "true" : "false") +
                            ",\"positionMs\":" + positionMs +
                            ",\"durationMs\":" + durationMs +
                            ",\"lastUpdatedMs\":" + lastUpdated +
                            ",\"platform\":\"" + EscapeJson(platform) +
                            "\",\"sourceApp\":\"" + EscapeJson(sourceApp) + "\"}");
                    }
                    catch (Exception ex)
                    {
                        Console.Error.WriteLine($"[SMTC-ERROR] {ex.Message}");
                        Console.WriteLine($$"""{"isPlaying":false,"psError":"{{EscapeJson(ex.Message)}}"}""");
                    }

                    await Task.Delay(1000);
                }
            }
            catch (Exception ex)
            {
                noSessionStreak = 0; // RequestAsync novo = manager novo
                Console.Error.WriteLine($"[SMTC-FATAL] {ex.Message}");
                Console.WriteLine($$"""{"isPlaying":false,"psFatal":"{{EscapeJson(ex.Message)}}"}""");
                // Delay reduzido de 5s para 2s para reconectar mais rápido
                await Task.Delay(2000);
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  SEEK MODE
    // ═══════════════════════════════════════════════════════════════════════

    /// <summary>
    /// Envia comando de seek via SMTC API e retorna true se bem-sucedido.
    /// Saída no stdout: "OK" ou mensagem de erro.
    /// </summary>
    static async Task<bool> SeekTrackAsync(long positionMs)
    {
        try
        {
            var manager = await GlobalSystemMediaTransportControlsSessionManager.RequestAsync();
            // Prefere a sessão do Spotify (mesma lógica do modo daemon) para não
            // buscar na app errada quando outra (ex: YouTube no navegador) é a
            // "sessão atual" do sistema.
            var session = PickSpotifySession(manager);

            if (session == null)
            {
                Console.Error.WriteLine("[SEEK] No Spotify media session");
                Console.WriteLine("FAIL:no-session");
                return false;
            }

            // TryChangePlaybackPositionAsync recebe long (ms), nao TimeSpan
            var result = await session.TryChangePlaybackPositionAsync(positionMs);

            if (result)
            {
                Console.WriteLine("OK");
                return true;
            }

            Console.Error.WriteLine("[SEEK] TryChangePlaybackPositionAsync returned false");
            Console.WriteLine("FAIL:rejected");
            return false;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[SEEK-ERROR] {ex.Message}");
            Console.WriteLine($"ERR:{EscapeJson(ex.Message)}");
            return false;
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  HELPERS
    // ═══════════════════════════════════════════════════════════════════════

    static string DetectPlatform(string sourceApp)
    {
        var lower = sourceApp.ToLowerInvariant();
        if (lower.Contains("spotify"))
            return "spotify";
        if (lower.Contains("apple"))
            return "apple-music";
        return "unknown";
    }

    /// <summary>
    /// Escolhe a sessão SMTC do Spotify a reportar.
    /// GetCurrentSession() aponta para a app que tocou áudio por último — com
    /// YouTube/outros tocando ao mesmo tempo, o daemon ficava alternando entre
    /// fontes a cada 1s e o app oscilava entre faixas. Aqui procuramos SEMPRE
    /// uma sessão do Spotify, na ordem:
    ///   1. A sessão atual, se for do Spotify (caso comum);
    ///   2. Senão, qualquer sessão do Spotify em GetSessions(), preferindo a que
    ///      está tocando (playing).
    /// Retorna null quando não existe nenhuma sessão do Spotify (fechado/parado).
    /// </summary>
    static GlobalSystemMediaTransportControlsSession? PickSpotifySession(
        GlobalSystemMediaTransportControlsSessionManager manager)
    {
        var current = manager.GetCurrentSession();
        if (current != null && IsSpotifySource(current.SourceAppUserModelId ?? ""))
            return current;

        GlobalSystemMediaTransportControlsSession? found = null;
        foreach (var s in manager.GetSessions())
        {
            if (!IsSpotifySource(s.SourceAppUserModelId ?? "")) continue;
            found ??= s;
            if (s.GetPlaybackInfo()?.PlaybackStatus
                == GlobalSystemMediaTransportControlsSessionPlaybackStatus.Playing)
            {
                return s;
            }
        }
        return found;
    }

    /// <summary>
    /// True se o processo pai (PID do backend) ainda está vivo. Quando o pai
    /// morre de forma que o Node não detecta (taskkill /F, crash), o daemon
    /// deve sair — senão vira um órfão monopolizando o SMTC.
    /// </summary>
    static bool IsParentAlive(int? parentPid)
    {
        if (parentPid is not int pid || pid <= 0)
            return true; // sem watchdog configurado (execução manual)

        try
        {
            using var proc = System.Diagnostics.Process.GetProcessById(pid);
            if (proc.HasExited)
                return false;
            // PID reciclado: se o processo que ocupa o PID começou DEPOIS do
            // daemon, não é o backend original (ele morreu e o Windows reusou
            // o PID). Sem isto o daemon ficaria vivo para sempre como órfão.
            return proc.StartTime.ToUniversalTime() <= SelfStartedUtc;
        }
        catch (ArgumentException)
        {
            return false; // processo não existe mais → pai morto
        }
        catch
        {
            return true; // sem acesso/permissão → assume vivo (não derruba o daemon)
        }
    }

    /// <summary>Momento de início deste daemon — usado para detectar PID reciclado.</summary>
    static readonly DateTime SelfStartedUtc = DateTime.UtcNow;

    /// <summary>True se a fonte SMTC pertence ao Spotify (Spotify.exe, AUMID da Store etc.).</summary>
    static bool IsSpotifySource(string sourceApp)
    {
        var lower = sourceApp.ToLowerInvariant();
        return lower.Contains("spotify")
            || lower.Contains("spotifyab")
            || lower.Contains("spotify.music");
    }

    /// <summary>
    /// Escapa strings para JSON válido. Além de \\, ", \n, \r, \t, escapa
    /// qualquer caractere de controle restante (< 0x20) como \uXXXX — o
    /// JsonSerializer antigo fazia isso e o JSON.parse do backend (spotify.js)
    /// rejeita control chars crus no meio da string.
    /// </summary>
    static string EscapeJson(string s)
    {
        var sb = new System.Text.StringBuilder(s.Length + 8);
        foreach (char c in s)
        {
            switch (c)
            {
                case '\\': sb.Append("\\\\"); break;
                case '"': sb.Append("\\\""); break;
                case '\n': sb.Append("\\n"); break;
                case '\r': sb.Append("\\r"); break;
                case '\t': sb.Append("\\t"); break;
                case '\b': sb.Append("\\b"); break;
                case '\f': sb.Append("\\f"); break;
                default:
                    if (c < 0x20)
                        sb.Append("\\u").Append(((int)c).ToString("x4"));
                    else
                        sb.Append(c);
                    break;
            }
        }
        return sb.ToString();
    }
}
