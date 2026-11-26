using LocalNotebook.Interop;
using LocalNotebook.Services;
using LocalNotebook.Views;
using Microsoft.UI.Xaml;
using System.IO;

namespace LocalNotebook;

public partial class App : Application
{
    public static CoreClient Core { get; } = new();
    private Window? _window;

    public App()
    {
        InitializeComponent();
        // A bug in one handler should show an error, not take the whole app down.
        UnhandledException += (_, e) =>
        {
            e.Handled = true;
            try
            {
                var dir = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "LocalNotebook", "logs");
                Directory.CreateDirectory(dir);
                File.AppendAllText(Path.Combine(dir, "ui-errors.log"), $"{DateTime.Now:O} {e.Exception}{Environment.NewLine}{Environment.NewLine}");
            }
            catch (IOException) { }
            if (_window is MainWindow mw) mw.DispatcherQueue.TryEnqueue(() => mw.ReportStartupFailure(L10n.Fmt("app.ui_error", e.Message)));
        };
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        // Section 14.1: Show window immediately, initialize Core asynchronously
        _window = new MainWindow();
        _window.Activate();

        // LNA_DATA_DIR points the app at a copy of the data (testing); unset in normal use.
        var appData = Environment.GetEnvironmentVariable("LNA_DATA_DIR") is { Length: > 0 } over
            ? over
            : Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "LocalNotebook");

        _ = Task.Run(() =>
        {
            try
            {
                Core.Open(appData);
            }
            catch (Exception ex)
            {
                // Without this the unobserved exception leaves a window that silently does nothing.
                if (_window is MainWindow failed)
                {
                    failed.DispatcherQueue.TryEnqueue(() => failed.ReportStartupFailure(ex.Message));
                }
                return;
            }
            if (_window is MainWindow mw)
            {
                mw.DispatcherQueue.TryEnqueue(async () => await mw.OnCoreReadyAsync());
            }
        });
    }
}
