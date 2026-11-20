using LocalNotebook.Interop;
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
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        // Section 14.1: Show window immediately, initialize Core asynchronously
        _window = new MainWindow();
        _window.Activate();

        var appData = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "LocalNotebook");

        _ = Task.Run(() =>
        {
            Core.Open(appData);
            if (_window is MainWindow mw)
            {
                mw.DispatcherQueue.TryEnqueue(async () => await mw.OnCoreReadyAsync());
            }
        });
    }
}
